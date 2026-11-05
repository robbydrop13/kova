//! The band at the top of the chat that says, at a glance, what the agent
//! is doing (KovaLink's readability rule 1): `Working · 1 min 12 s`,
//! `Waiting for you`, `Done 3 min ago`, `Offline`.

use serde::Serialize;

/// What the pane says about its agent, read on the tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BannerInput {
    /// A Claude session runs in the pane.
    pub has_session: bool,
    pub working: bool,
    /// A permission prompt on screen, or a `kova ask` of this pane on the board.
    pub waiting: bool,
    /// When the turn in flight started, epoch seconds (the user's last
    /// message, else the moment the chat saw it working).
    pub working_since: Option<u64>,
    /// When the agent last did something, epoch seconds.
    pub last_activity: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Banner {
    /// The page counts the time from `since` itself, every second.
    Working { since: Option<u64> },
    Waiting,
    Done { at: Option<u64> },
    Offline,
}

/// The banner for what the pane says. A question waiting wins over the
/// spinner: a permission prompt keeps the title of a working Claude for a
/// moment, and the answer is what the user must give.
pub fn banner(input: BannerInput) -> Banner {
    if !input.has_session {
        return Banner::Offline;
    }
    if input.waiting {
        return Banner::Waiting;
    }
    if input.working {
        return Banner::Working { since: input.working_since };
    }
    Banner::Done { at: input.last_activity }
}

/// `1 min 12 s`, `45 s`, `2 h 03 min`: the duration the banner shows.
pub fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// `just now`, `3 min ago`, `2 h ago`, `3 d ago`.
pub fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86_400),
    }
}

/// The banner's text at `now`. The page has the same rule in its script, to
/// tick the clock without a round trip; this one is the reference the
/// tests pin.
pub fn text(banner: Banner, now: u64) -> String {
    match banner {
        Banner::Working { since: Some(t) } => format!("Working · {}", duration(now.saturating_sub(t))),
        Banner::Working { since: None } => "Working".to_string(),
        Banner::Waiting => "Waiting for you".to_string(),
        Banner::Done { at: Some(t) } => format!("Done {}", ago(now.saturating_sub(t))),
        Banner::Done { at: None } => "Done".to_string(),
        Banner::Offline => "Offline".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> BannerInput {
        BannerInput { has_session: true, ..Default::default() }
    }

    #[test]
    fn no_session_is_offline_whatever_else_holds() {
        assert_eq!(banner(BannerInput { working: true, waiting: true, ..Default::default() }), Banner::Offline);
    }

    #[test]
    fn a_question_wins_over_the_spinner() {
        assert_eq!(banner(BannerInput { working: true, waiting: true, ..input() }), Banner::Waiting);
    }

    #[test]
    fn working_counts_from_the_turn_start() {
        let b = banner(BannerInput { working: true, working_since: Some(1000), ..input() });
        assert_eq!(text(b, 1072), "Working · 1 min 12 s");
        assert_eq!(text(Banner::Working { since: Some(0) }, 7380), "Working · 2 h 03 min");
    }

    #[test]
    fn idle_says_how_long_ago_it_finished() {
        let b = banner(BannerInput { last_activity: Some(1000), ..input() });
        assert_eq!(text(b, 1000 + 180), "Done 3 min ago");
        assert_eq!(text(b, 1010), "Done just now");
        assert_eq!(text(Banner::Done { at: None }, 5), "Done");
    }

    #[test]
    fn the_page_gets_the_state_and_the_clock_origin() {
        let json = serde_json::to_value(Banner::Working { since: Some(5) }).unwrap();
        assert_eq!(json, serde_json::json!({"state": "working", "since": 5}));
        assert_eq!(serde_json::to_value(Banner::Offline).unwrap(), serde_json::json!({"state": "offline"}));
    }
}
