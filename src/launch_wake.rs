//! Wake every agent a launch restore left asleep, one at a time.
//!
//! A relaunch restores agent panes asleep (`Surface::sleep_until_shown`) so
//! fifty conversations do not all start at once. With `[session]
//! wake_on_launch`, the app wakes them itself after the launch, as if each tab
//! had been clicked, but `wake_stagger_s` apart: the Cockpit's chief first,
//! then the other chief, then every other pane in window, tab and pane order.
//! Each wake goes through `Surface::wake`, like `wake-pane`: no focus, no tab
//! switch. This file is the pure half (who comes next, and when); the app
//! timer gathers the panes and does the waking.

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// One pane of one window, as the waker sees it.
#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub pane_id: u32,
    /// The conversation asleep in the pane (`Surface::dormant_session_id`).
    pub dormant: Option<String>,
    /// The conversation an agent runs in the pane (`Surface::agent_session_id`).
    pub live: Option<String>,
    /// 0 for the chief of its window's Cockpit, 1 for the other chief, 2 else.
    pub rank: u8,
}

/// The rank of a pane named by `titles` (its restored OSC title, its custom
/// title): a Claude conversation's OSC title is its name, so a sleeping chief
/// still says `claap-agent`, behind a leading status glyph when it had one.
pub fn rank(titles: &[Option<&str>], cockpit_chief: &str, chiefs: &[&str]) -> u8 {
    let names: Vec<&str> = titles
        .iter()
        .flatten()
        .map(|t| t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim())
        .collect();
    if names.contains(&cockpit_chief) {
        0
    } else if chiefs.iter().any(|c| names.contains(c)) {
        1
    } else {
        2
    }
}

/// The next pane to wake, `panes` in window, tab and pane order: the lowest
/// rank, first in order, among the sleeping panes not yet woken whose
/// conversation no pane runs and no earlier wake resumed.
pub fn next_to_wake(panes: &[PaneInfo], woken_panes: &HashSet<u32>, woken_sessions: &HashSet<String>) -> Option<(u32, String)> {
    let live: HashSet<&str> = panes.iter().filter_map(|p| p.live.as_deref()).collect();
    panes
        .iter()
        .filter(|p| !woken_panes.contains(&p.pane_id))
        .filter_map(|p| p.dormant.as_deref().map(|id| (p, id)))
        .filter(|(_, id)| !live.contains(id) && !woken_sessions.contains(*id))
        .min_by_key(|(p, _)| p.rank)
        .map(|(p, id)| (p.pane_id, id.to_string()))
}

/// What the app timer keeps between ticks.
pub struct LaunchWaker {
    stagger: Duration,
    next_at: Instant,
    woken_panes: HashSet<u32>,
    woken_sessions: HashSet<String>,
}

impl LaunchWaker {
    pub fn new(stagger_s: f64, now: Instant) -> Self {
        Self {
            stagger: Duration::from_secs_f64(stagger_s.max(0.0)),
            next_at: now,
            woken_panes: HashSet::new(),
            woken_sessions: HashSet::new(),
        }
    }

    /// Whether a wake may happen now.
    pub fn due(&self, now: Instant) -> bool {
        now >= self.next_at
    }

    /// The pane to wake now, `None` when nothing waits.
    pub fn pick(&self, panes: &[PaneInfo]) -> Option<(u32, String)> {
        next_to_wake(panes, &self.woken_panes, &self.woken_sessions)
    }

    /// `pane_id` was handled (woken, or found awake): never again, and the
    /// next one waits a stagger.
    pub fn record(&mut self, pane_id: u32, session: String, now: Instant) {
        self.woken_panes.insert(pane_id);
        self.woken_sessions.insert(session);
        self.next_at = now + self.stagger;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHIEFS: [&str; 2] = ["claap-agent", "perso-agent"];

    fn asleep(pane_id: u32, id: &str, rank: u8) -> PaneInfo {
        PaneInfo { pane_id, dormant: Some(id.into()), live: None, rank }
    }

    fn running(pane_id: u32, id: &str) -> PaneInfo {
        PaneInfo { pane_id, dormant: None, live: Some(id.into()), rank: 2 }
    }

    fn order(panes: &[PaneInfo]) -> Vec<u32> {
        let mut waker = LaunchWaker::new(3.0, Instant::now());
        let mut out = Vec::new();
        while let Some((id, session)) = waker.pick(panes) {
            out.push(id);
            waker.record(id, session, Instant::now());
        }
        out
    }

    #[test]
    fn the_cockpit_chief_then_the_other_chief_then_tab_order() {
        let panes = [asleep(1, "a", 2), asleep(2, "perso", 1), asleep(3, "b", 2), asleep(4, "pro", 0)];
        assert_eq!(order(&panes), vec![4, 2, 1, 3]);
    }

    #[test]
    fn a_session_running_elsewhere_or_already_resumed_is_skipped() {
        let panes = [asleep(1, "a", 2), running(2, "a"), asleep(3, "b", 2), asleep(4, "b", 2)];
        assert_eq!(order(&panes), vec![3]);
    }

    #[test]
    fn a_pane_that_woke_by_itself_is_left_alone() {
        let mut waker = LaunchWaker::new(3.0, Instant::now());
        let mut panes = vec![asleep(1, "a", 2), asleep(2, "b", 2)];
        // The user clicked pane 1's tab: it is no longer asleep, its agent runs.
        panes[0] = running(1, "a");
        assert_eq!(waker.pick(&panes), Some((2, "b".into())));
        waker.record(2, "b".into(), Instant::now());
        assert_eq!(waker.pick(&panes), None);
    }

    #[test]
    fn wakes_are_a_stagger_apart() {
        let start = Instant::now();
        let mut waker = LaunchWaker::new(3.0, start);
        assert!(waker.due(start));
        waker.record(1, "a".into(), start);
        assert!(!waker.due(start + Duration::from_millis(2999)));
        assert!(waker.due(start + Duration::from_secs(3)));
    }

    #[test]
    fn a_chief_is_known_by_its_restored_title_behind_a_status_glyph() {
        assert_eq!(rank(&[Some("✳ claap-agent"), Some("claude")], "claap-agent", &CHIEFS), 0);
        assert_eq!(rank(&[Some("claap-agent"), None], "perso-agent", &CHIEFS), 1);
        assert_eq!(rank(&[Some("claapagent-palette"), Some("claude")], "claap-agent", &CHIEFS), 2);
        assert_eq!(rank(&[None, None], "claap-agent", &CHIEFS), 2);
    }
}
