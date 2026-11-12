//! `kova doctor`: who signed the running Kova, and what macOS lets it do.
//!
//! macOS ties a grant of Privacy & Security to the certificate that signed
//! the app. A build signed with another certificate keeps its switch ON in
//! System Settings and loses the grant all the same, so the switch proves
//! nothing: only the process itself can say whether it is trusted. The
//! running Kova answers the `doctor` IPC command with that; `kova doctor`
//! asks it and prints the answer.

use serde_json::{Value, json};
use std::process::Command;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// What this process is trusted for, asked without prompting the user: the
/// key the report carries, the name System Settings shows, the answer.
pub fn permissions() -> [(&'static str, &'static str, bool); 4] {
    // SAFETY: four argument-less preflight calls, safe from any thread.
    unsafe {
        [
            ("accessibility", "Accessibility", AXIsProcessTrusted()),
            ("post_event", "Accessibility, posting events (keys, clicks)", CGPreflightPostEventAccess()),
            ("listen_event", "Input Monitoring", CGPreflightListenEventAccess()),
            ("screen_capture", "Screen & System Audio Recording", CGPreflightScreenCaptureAccess()),
        ]
    }
}

/// The certificate that signed `target` (a path, or `+<pid>` for a running
/// process): its SHA-1 in upper-case hex and its name, as `codesign` reads
/// them. `None` for an unsigned or ad hoc signed target.
pub fn signing_leaf(target: &str) -> Option<(String, String)> {
    let dir = std::env::temp_dir().join(format!("kova-doctor-{}-{}", std::process::id(), crate::ask::new_question_id()));
    std::fs::create_dir_all(&dir).ok()?;
    let prefix = dir.join("c");
    let leaf = (|| {
        let out = Command::new("/usr/bin/codesign")
            .arg("-d")
            .arg(format!("--extract-certificates={}", prefix.display()))
            .arg(target)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let sum = Command::new("/usr/bin/shasum").args(["-a", "1"]).arg(dir.join("c0")).output().ok()?;
        let sha = String::from_utf8_lossy(&sum.stdout).split_whitespace().next()?.to_ascii_uppercase();
        (sha.len() == 40).then_some(sha)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let sha = leaf?;
    let shown = Command::new("/usr/bin/codesign").args(["-dvv", target]).output().ok()?;
    Some((sha, authority(&String::from_utf8_lossy(&shown.stderr)).unwrap_or_default()))
}

/// The first `Authority=` of `codesign -dvv`: the signing certificate's name.
pub fn authority(shown: &str) -> Option<String> {
    shown.lines().find_map(|l| l.strip_prefix("Authority=")).map(|a| a.trim().to_string())
}

/// The identity `build.sh` signed the last build with, from its state file
/// (`<SHA-1> <name>`): what the next build will sign with.
pub fn recorded_identity(text: &str) -> Option<(String, String)> {
    let line = text.lines().next()?.trim();
    let (sha, name) = line.split_once(' ').unwrap_or((line, ""));
    (sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())).then(|| (sha.to_ascii_uppercase(), name.trim().to_string()))
}

fn state_file() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("KOVA_SIGN_STATE") {
        return Some(path.into());
    }
    Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config/kova/sign-identity"))
}

/// This process, for the `doctor` IPC command: its executable, the
/// certificate it runs under (the one of its launch, whatever was built
/// over the bundle since) and what it is trusted for.
pub fn report() -> Value {
    let pid = std::process::id();
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let leaf = signing_leaf(&format!("+{pid}"));
    let mut permissions_json = serde_json::Map::new();
    for (key, _, trusted) in permissions() {
        permissions_json.insert(key.to_string(), json!(trusted));
    }
    json!({
        "pid": pid,
        "version": env!("CARGO_PKG_VERSION"),
        "exe": exe,
        "signing": leaf.map(|(sha, name)| json!({"sha1": sha, "name": name})),
        "permissions": permissions_json,
    })
}

/// The lines `kova doctor` prints for a report: one per fact, `NOT TRUSTED`
/// in capitals, then what to do when the signature explains it.
pub fn lines(report: &Value, on_disk: Option<&(String, String)>, recorded: Option<&(String, String)>) -> Vec<String> {
    let word = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let label = |sha: &str, name: &str| if name.is_empty() { sha.to_string() } else { format!("{sha} \"{name}\"") };
    let mut out = Vec::new();
    out.push(format!(
        "Kova {} (pid {}) {}",
        word(report, "version"),
        report.get("pid").and_then(Value::as_u64).unwrap_or(0),
        word(report, "exe")
    ));
    let running = report.get("signing").filter(|s| !s.is_null()).map(|s| (word(s, "sha1"), word(s, "name")));
    out.push(match &running {
        Some((sha, name)) => format!("signed by   {}", label(sha, name)),
        None => "signed by   nothing (unsigned or ad hoc): macOS keeps no grant across builds".to_string(),
    });
    if let Some((sha, name)) = on_disk
        && running.as_ref().map(|r| &r.0) != Some(sha)
    {
        out.push(format!("on disk     {}  <- DIFFERS: the bundle was signed otherwise since this Kova started", label(sha, name)));
    }
    if let Some((sha, name)) = recorded
        && running.as_ref().map(|r| &r.0) != Some(sha)
    {
        out.push(format!("build.sh    {}  <- DIFFERS: the next build signs with this one", label(sha, name)));
    }
    let mut refused = 0;
    for (key, shown, _) in permissions() {
        let trusted = report.get("permissions").and_then(|p| p.get(key)).and_then(Value::as_bool);
        if trusted == Some(false) {
            refused += 1;
        }
        out.push(format!(
            "{:<15} {}  ({shown})",
            key,
            match trusted {
                Some(true) => "trusted    ",
                Some(false) => "NOT TRUSTED",
                None => "unknown    ",
            }
        ));
    }
    if refused > 0 {
        out.push(
            "A switch that is ON in System Settings > Privacy & Security with NOT TRUSTED here is a grant made for another signature: remove Kova from that list and add it again."
                .to_string(),
        );
    }
    out
}

pub const USAGE: &str = "kova doctor [--socket <path>] [--json]

Who signed the running Kova and what macOS lets it do, as that process sees it:
Accessibility, posting events, Input Monitoring, Screen Recording. Asked to the
running Kova ($KOVA_SOCKET, else the running Kova.app). Exit 0 when everything
is trusted, 1 when something is not, 2 when no running Kova answered.
Only the running Kova can answer for itself: one started before this command
existed must be relaunched first.";

/// `kova doctor`: returns the exit code.
pub fn run_cli(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let json_out = args.iter().any(|a| a == "--json");
    let socket = args.windows(2).find(|w| w[0] == "--socket").map(|w| w[1].clone());
    let on_disk = signing_leaf("/Applications/Kova.app");
    let recorded = state_file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| recorded_identity(&t));
    let Some(report) = crate::ask::doctor_report(socket) else {
        eprintln!("kova doctor: no running Kova answered `doctor` (none running, or one started before this command existed: relaunch it).");
        if let Some((sha, name)) = &on_disk {
            eprintln!("on disk     {sha} \"{name}\"  (/Applications/Kova.app)");
        }
        if let Some((sha, name)) = &recorded {
            eprintln!("build.sh    {sha} \"{name}\"");
        }
        return 2;
    };
    if json_out {
        println!("{report}");
    } else {
        for line in lines(&report, on_disk.as_ref(), recorded.as_ref()) {
            println!("{line}");
        }
    }
    let refused = report.get("permissions").and_then(Value::as_object).is_some_and(|p| p.values().any(|v| v == &json!(false)));
    if refused { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_names_the_four_grants_and_this_process() {
        let report = report();
        assert_eq!(report["pid"], json!(std::process::id()));
        let permissions = report["permissions"].as_object().unwrap();
        assert_eq!(permissions.len(), 4);
        for key in ["accessibility", "post_event", "listen_event", "screen_capture"] {
            assert!(permissions[key].is_boolean(), "{key}");
        }
    }

    #[test]
    fn a_grant_lost_to_another_signature_reads_in_capitals_with_what_to_do() {
        let report = json!({
            "pid": 7, "version": "1.0.0", "exe": "/Applications/Kova.app/Contents/MacOS/kova",
            "signing": {"sha1": "A".repeat(40), "name": "New Cert"},
            "permissions": {"accessibility": false, "post_event": false, "listen_event": true, "screen_capture": true},
        });
        let old = ("B".repeat(40), "Old Cert".to_string());
        let text = lines(&report, Some(&old), Some(&old)).join("\n");
        assert!(text.contains(&format!("signed by   {} \"New Cert\"", "A".repeat(40))), "{text}");
        assert!(text.contains("accessibility   NOT TRUSTED"), "{text}");
        assert!(text.contains("listen_event    trusted"), "{text}");
        assert!(text.contains("on disk     ") && text.contains("build.sh    ") && text.matches("DIFFERS").count() == 2, "{text}");
        assert!(text.contains("remove Kova from that list and add it again"), "{text}");
        // Everything trusted, same certificate everywhere: nothing to do, nothing more said.
        let fine = json!({
            "pid": 7, "version": "1.0.0", "exe": "/k",
            "signing": {"sha1": "B".repeat(40), "name": "Old Cert"},
            "permissions": {"accessibility": true, "post_event": true, "listen_event": true, "screen_capture": true},
        });
        let text = lines(&fine, Some(&old), Some(&old)).join("\n");
        assert!(!text.contains("DIFFERS") && !text.contains("NOT TRUSTED") && !text.contains("remove Kova"), "{text}");
        // An unsigned Kova says so.
        let bare = json!({"pid": 7, "version": "1", "exe": "/k", "signing": null, "permissions": {}});
        assert!(lines(&bare, None, None).join("\n").contains("signed by   nothing"));
    }

    #[test]
    fn the_recorded_identity_is_a_sha_and_a_name() {
        let sha = "2482232ab8caf625fa5fb0f35cff40ecb6e4f4a5";
        assert_eq!(recorded_identity(&format!("{sha} Some Cert: Name (X)\n")), Some((sha.to_ascii_uppercase(), "Some Cert: Name (X)".to_string())));
        assert_eq!(recorded_identity(sha), Some((sha.to_ascii_uppercase(), String::new())));
        assert_eq!(recorded_identity("not a sha"), None);
        assert_eq!(recorded_identity(""), None);
        assert_eq!(authority("Executable=/k\nAuthority=Leaf Cert\nAuthority=Root\n"), Some("Leaf Cert".to_string()));
        assert_eq!(authority("Signature=adhoc\n"), None);
    }
}
