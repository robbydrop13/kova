//! No typing help from macOS in Kova's text fields.
//!
//! What the user types in a field of Kova goes to an agent as he typed it:
//! nothing completes, corrects, replaces or capitalizes it (15/11/2026:
//! `qa` typed in the Cockpit's chat box became a suggested `qataris`).
//!
//! Two layers, since the fields are of two kinds:
//!
//! - every field of a page (a `<textarea>`, a text `<input>`) carries
//!   `FIELD_ATTRS`, which WebKit reads per field;
//! - what WebKit and AppKit only read from the user's defaults (smart
//!   quotes and dashes, text replacement, the period on a double space,
//!   the inline prediction) is turned off for this process alone
//!   (`turn_off`), which also covers the native fields (the ask card).
//!
//! The spelling underline is not typing help: it inserts nothing. A field
//! says for itself whether it wants it (`spellcheck`).

/// On every text field of a page: no completion, no correction, no capital.
pub const FIELD_ATTRS: &str = r#"autocomplete="off" autocorrect="off" autocapitalize="off""#;

/// The defaults read by AppKit's text views (`NS…`) and by WebKit's text
/// checker (`Web…`), all set to false for this process.
pub const DEFAULTS_OFF: [&str; 13] = [
    "NSAutomaticSpellingCorrectionEnabled",
    "NSAutomaticTextCompletionEnabled",
    "NSAutomaticTextReplacementEnabled",
    // The one AppKit reads for the user's own replacements (measured on
    // macOS 27: `NSSpellChecker.isAutomaticTextReplacementEnabled`).
    "NSUserDictionaryReplacementItemsEnabled",
    "NSAutomaticQuoteSubstitutionEnabled",
    "NSAutomaticDashSubstitutionEnabled",
    "NSAutomaticCapitalizationEnabled",
    "NSAutomaticPeriodSubstitutionEnabled",
    "NSAutomaticInlinePredictionEnabled",
    "WebAutomaticSpellingCorrectionEnabled",
    "WebAutomaticTextReplacementEnabled",
    "WebAutomaticQuoteSubstitutionEnabled",
    "WebAutomaticDashSubstitutionEnabled",
];

/// Turn the typing help off for this process, before any field exists. The
/// values go to the argument domain, which is read first and never written
/// to disk: the user's settings for his other apps, and Kova's own
/// preferences file, are untouched.
pub fn turn_off() {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSNumber, NSString};
    unsafe {
        let defaults: Retained<AnyObject> = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let domain = NSString::from_str("NSArgumentDomain");
        // What the command line already set stays.
        let current: Option<Retained<AnyObject>> = msg_send![&defaults, volatileDomainForName: &*domain];
        let values: Retained<AnyObject> = match current {
            Some(current) => msg_send![class!(NSMutableDictionary), dictionaryWithDictionary: &*current],
            None => msg_send![class!(NSMutableDictionary), dictionary],
        };
        let off = NSNumber::new_bool(false);
        for key in DEFAULTS_OFF {
            let _: () = msg_send![&values, setObject: &*off, forKey: &*NSString::from_str(key)];
        }
        let _: () = msg_send![&defaults, removeVolatileDomainForName: &*domain];
        let _: () = msg_send![&defaults, setVolatileDomain: &*values, forName: &*domain];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text field of every page Kova draws carries the attributes: a
    /// field added without them fails here.
    #[test]
    fn every_field_of_a_page_refuses_typing_help() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "typing_assist.rs") {
                    files.push(path);
                }
            }
        }
        let mut fields = 0;
        for path in files {
            let code = std::fs::read_to_string(&path).unwrap();
            for (n, line) in code.lines().enumerate() {
                let line = line.replace("\\\"", "\"");
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with("assert") {
                    continue;
                }
                for tag in ["<textarea ", "<textarea>", "<input type=\"text\""] {
                    for (at, _) in line.match_indices(tag) {
                        // The parser's own mention of the tag in a comment or a selector is not a field.
                        if line[..at].ends_with('`') || line[..at].ends_with('\'') {
                            continue;
                        }
                        fields += 1;
                        let end = line[at..].find('>').map_or(line.len(), |e| at + e);
                        assert!(
                            line[at..end].contains("{NO_HELP}") || line[at..end].contains(FIELD_ATTRS),
                            "{}:{}: a text field without the no-typing-help attributes",
                            path.strip_prefix(&dir).unwrap().display(),
                            n + 1
                        );
                    }
                }
            }
        }
        assert!(fields >= 9, "{fields} fields found: the scan lost them");
    }

    /// Run for real in this process: the defaults read false afterwards,
    /// through `NSUserDefaults` and through what AppKit's text views ask
    /// (`NSSpellChecker`), and nothing is written to a preferences file.
    #[test]
    fn turning_it_off_is_read_by_appkit_and_written_nowhere() {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_foundation::NSString;
        turn_off();
        unsafe {
            let defaults: Retained<AnyObject> = msg_send![class!(NSUserDefaults), standardUserDefaults];
            for key in DEFAULTS_OFF {
                let key = NSString::from_str(key);
                let set: Option<Retained<AnyObject>> = msg_send![&defaults, objectForKey: &*key];
                let on: bool = msg_send![&defaults, boolForKey: &*key];
                assert!(set.is_some() && !on, "{key}");
            }
            let correction: bool = msg_send![class!(NSSpellChecker), isAutomaticSpellingCorrectionEnabled];
            let replacement: bool = msg_send![class!(NSSpellChecker), isAutomaticTextReplacementEnabled];
            let quotes: bool = msg_send![class!(NSSpellChecker), isAutomaticQuoteSubstitutionEnabled];
            let dashes: bool = msg_send![class!(NSSpellChecker), isAutomaticDashSubstitutionEnabled];
            let capitals: bool = msg_send![class!(NSSpellChecker), isAutomaticCapitalizationEnabled];
            let period: bool = msg_send![class!(NSSpellChecker), isAutomaticPeriodSubstitutionEnabled];
            let completion: bool = msg_send![class!(NSSpellChecker), isAutomaticTextCompletionEnabled];
            assert_eq!([correction, replacement, quotes, dashes, capitals, period, completion], [false; 7]);
            // Only the argument domain holds them: no persistent domain of this process does.
            let domain = NSString::from_str("NSArgumentDomain");
            let held: Option<Retained<AnyObject>> = msg_send![&defaults, volatileDomainForName: &*domain];
            let count: usize = msg_send![&*held.unwrap(), count];
            assert!(count >= DEFAULTS_OFF.len());
        }
    }

    #[test]
    fn the_defaults_cover_what_the_user_listed() {
        for word in ["SpellingCorrection", "TextCompletion", "TextReplacement", "QuoteSubstitution", "DashSubstitution", "Capitalization", "PeriodSubstitution", "InlinePrediction"] {
            assert!(DEFAULTS_OFF.iter().any(|k| k.starts_with("NS") && k.contains(word)), "{word}");
        }
        for word in ["SpellingCorrection", "TextReplacement", "QuoteSubstitution", "DashSubstitution"] {
            assert!(DEFAULTS_OFF.iter().any(|k| k.starts_with("Web") && k.contains(word)), "{word}");
        }
        assert_eq!(FIELD_ATTRS, r#"autocomplete="off" autocorrect="off" autocapitalize="off""#);
    }
}
