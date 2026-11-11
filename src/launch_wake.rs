//! Wake every agent a launch restore left asleep, a few at a time.
//!
//! Every restored agent is woken (the user wants them all open). Since
//! 11/11/2026 the pace is no longer a fixed 3 s clock: `batch` agents start
//! together, and the next one starts as soon as an earlier one shows its
//! prompt (or after `ready_timeout`, so a stuck one never holds the line).
//! The order: the Cockpit's chief, the other chief, the sessions with open
//! work (`Wanted::Named`, read once from the queue by
//! `inflight::open_owners`), then the others in window, tab and pane order.
//! Each launch logs when each agent was ready and when the last one was.
//!
//! A relaunch restores agent panes asleep (`Surface::sleep_until_shown`) so
//! fifty conversations do not all start at once. With `[session]
//! wake_on_launch`, the app wakes them itself after the launch, as if each tab
//! had been clicked, but `wake_stagger_s` apart: the Cockpit's chief first,
//! then the other chief, then every other pane in window, tab and pane order.
//! Each wake goes through `Surface::wake`, like `wake-pane`: no focus, no tab
//! switch. This file is the pure half (who comes next, and when); the app
//! timer gathers the panes and does the waking.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// One pane of one window, as the waker sees it.
#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub pane_id: u32,
    /// The conversation asleep in the pane (`Surface::dormant_session_id`).
    pub dormant: Option<String>,
    /// The conversation an agent runs in the pane (`Surface::agent_session_id`),
    /// or the one just woken there whose agent has not registered yet.
    pub live: Option<String>,
    /// 0 for the chief of its window's Cockpit, 1 for the other chief, 2 else.
    /// Only read while the pane sleeps.
    pub rank: u8,
    /// The session's names as the pane says them (its restored title, its
    /// custom title), cleaned like `rank` does: what `Wanted::Named` matches.
    pub names: Vec<String>,
    /// A pane that runs: its agent shows its prompt (or works). Its start is over.
    pub ready: bool,
}

/// A pane's names from its titles, without the leading status glyph.
pub fn names(titles: &[Option<&str>]) -> Vec<String> {
    titles.iter().flatten().map(|t| t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// Who goes right after the two chiefs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// The queue has not answered yet: the chiefs go meanwhile.
    Pending,
    /// The sessions with open work, by name: before the others. Empty when
    /// the queue did not answer in time: tab order after the chiefs.
    Named(HashSet<String>),
}

/// An agent woken became ready (or ran out of time): for the launch's log.
#[derive(Debug, Clone, PartialEq)]
pub struct Ready {
    pub pane_id: u32,
    /// Seconds since the launch's start, and since its own wake.
    pub at: f64,
    pub took: f64,
    pub timed_out: bool,
}

/// What one turn of the waker says to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Wait,
    Wake(u32, String),
    /// Every restored agent is awake: how many were woken, how many of them
    /// ran out of time, and how long after the start the last one was ready.
    Done { woken: usize, timed_out: usize, secs: f64 },
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
    /// Each pane's rank the first time it was seen asleep: the title it is
    /// read from is the restored one only until the shell sets its own.
    ranks: HashMap<u32, u8>,
    /// The same for its names.
    seen_names: HashMap<u32, Vec<String>>,
    wanted: Wanted,
    /// Woken, not ready yet: since when.
    starting: HashMap<u32, Instant>,
    started: Instant,
    woken: usize,
    last_ready: Instant,
    batch: usize,
    ready_timeout: Duration,
    timed_out: usize,
    /// Agents that became ready since the caller last took them (`take_ready`).
    ready_log: Vec<Ready>,
}

/// How long an empty pick waits before looking again, while restored tabs
/// are still spawning.
const IDLE: Duration = Duration::from_millis(500);

impl LaunchWaker {
    pub fn new(stagger_s: f64, now: Instant) -> Self {
        Self {
            stagger: Duration::from_secs_f64(if stagger_s.is_finite() { stagger_s.clamp(0.0, 3600.0) } else { 3.0 }),
            next_at: now,
            woken_panes: HashSet::new(),
            woken_sessions: HashSet::new(),
            ranks: HashMap::new(),
            seen_names: HashMap::new(),
            wanted: Wanted::Named(HashSet::new()),
            starting: HashMap::new(),
            started: now,
            woken: 0,
            last_ready: now,
            batch: 3,
            ready_timeout: Duration::from_secs(40),
            timed_out: 0,
            ready_log: Vec::new(),
        }
    }

    /// `batch` agents start together (1 to 16); one that shows no prompt
    /// after `ready_timeout_s` (5 s to 10 min) frees its slot.
    pub fn paced(mut self, batch: usize, ready_timeout_s: f64) -> Self {
        self.batch = batch.clamp(1, 16);
        self.ready_timeout = Duration::from_secs_f64(if ready_timeout_s.is_finite() { ready_timeout_s.clamp(5.0, 600.0) } else { 40.0 });
        self
    }

    pub fn take_ready(&mut self) -> Vec<Ready> {
        std::mem::take(&mut self.ready_log)
    }

    /// A waker that waits for the queue to say who has open work.
    pub fn selective(stagger_s: f64, now: Instant) -> Self {
        Self { wanted: Wanted::Pending, ..Self::new(stagger_s, now) }
    }

    /// The queue answered (or did not, in time: an empty set).
    pub fn set_wanted(&mut self, names: HashSet<String>) {
        if self.wanted == Wanted::Pending {
            self.wanted = Wanted::Named(names);
        }
    }

    pub fn pending(&self) -> bool {
        self.wanted == Wanted::Pending
    }

    /// A sleeping pane's place in the line: 0 and 1 the chiefs, 2 a session
    /// with open work, 3 the others. `None`: not yet (the queue has not said).
    fn place(&self, p: &PaneInfo) -> Option<u8> {
        let rank = self.ranks.get(&p.pane_id).copied().unwrap_or(p.rank);
        match &self.wanted {
            _ if rank < 2 => Some(rank),
            Wanted::Pending => None,
            Wanted::Named(names) => Some(if self.seen_names.get(&p.pane_id).unwrap_or(&p.names).iter().any(|n| names.contains(n)) { 2 } else { 3 }),
        }
    }

    /// One turn: free the slots of the agents now ready, then wake the next
    /// one in line if a slot is free. `deferred`: tabs of the restored
    /// session are still to spawn. The caller wakes the pane, then calls
    /// `started_one` and `record`.
    pub fn step(&mut self, panes: &[PaneInfo], deferred: bool, now: Instant) -> Step {
        for p in panes.iter().filter(|p| p.dormant.is_some()) {
            self.ranks.entry(p.pane_id).or_insert(p.rank);
            self.seen_names.entry(p.pane_id).or_insert_with(|| p.names.clone());
        }
        let (started, timeout) = (self.started, self.ready_timeout);
        let mut freed = Vec::new();
        self.starting.retain(|id, since| {
            let ready = panes.iter().any(|p| p.pane_id == *id && p.dormant.is_none() && p.ready);
            let late = now.duration_since(*since) >= timeout;
            if ready || late {
                freed.push(Ready { pane_id: *id, at: now.duration_since(started).as_secs_f64(), took: now.duration_since(*since).as_secs_f64(), timed_out: !ready });
            }
            !(ready || late)
        });
        if !freed.is_empty() {
            self.last_ready = now;
            self.timed_out += freed.iter().filter(|r| r.timed_out).count();
            freed.sort_by_key(|r| r.pane_id);
            self.ready_log.extend(freed);
        }
        if !self.due(now) || self.starting.len() >= self.batch {
            return Step::Wait;
        }
        let among: Vec<PaneInfo> = panes
            .iter()
            .filter_map(|p| match p.dormant {
                Some(_) => self.place(p).map(|rank| PaneInfo { rank, ..p.clone() }),
                None => Some(p.clone()),
            })
            .collect();
        if let Some((pane_id, session)) = next_to_wake(&among, &self.woken_panes, &self.woken_sessions) {
            return Step::Wake(pane_id, session);
        }
        if deferred || self.pending() || !self.starting.is_empty() {
            self.idle(now);
            return Step::Wait;
        }
        Step::Done { woken: self.woken, timed_out: self.timed_out, secs: self.last_ready.duration_since(self.started).as_secs_f64() }
    }

    /// `pane_id` was really woken by this waker: it holds a slot until ready.
    pub fn started_one(&mut self, pane_id: u32, now: Instant) {
        self.starting.insert(pane_id, now);
        self.woken += 1;
        self.last_ready = now;
    }

    /// Whether a wake may happen now.
    pub fn due(&self, now: Instant) -> bool {
        now >= self.next_at
    }

    /// The pane to wake now, `None` when nothing waits. Ranks are the ones
    /// first seen.
    pub fn pick(&mut self, panes: &[PaneInfo]) -> Option<(u32, String)> {
        let panes: Vec<PaneInfo> = panes
            .iter()
            .map(|p| match p.dormant {
                Some(_) => PaneInfo { rank: *self.ranks.entry(p.pane_id).or_insert(p.rank), ..p.clone() },
                None => p.clone(),
            })
            .collect();
        next_to_wake(&panes, &self.woken_panes, &self.woken_sessions)
    }

    /// Nothing to wake yet: look again shortly, not on every frame.
    pub fn idle(&mut self, now: Instant) {
        self.next_at = now + IDLE;
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
        PaneInfo { pane_id, dormant: Some(id.into()), live: None, rank, names: vec![id.to_string()], ready: false }
    }

    fn running(pane_id: u32, id: &str) -> PaneInfo {
        PaneInfo { pane_id, dormant: None, live: Some(id.into()), rank: 2, names: Vec::new(), ready: false }
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

    /// A launch wakes everyone: the chiefs, the sessions with open work, then
    /// the others, `batch` at a time, the next when one is ready.
    #[test]
    fn a_launch_wakes_everyone_those_with_work_first_a_batch_at_a_time() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut waker = LaunchWaker::selective(0.0, t0).paced(2, 45.0);
        let mut panes = vec![asleep(1, "pro", 0), asleep(2, "perso", 1), asleep(3, "idle-one", 2), asleep(4, "night-qa", 2), asleep(5, "idle-two", 2), asleep(6, "lem-prd", 2)];
        let mut order = Vec::new();
        let mut turn = |waker: &mut LaunchWaker, panes: &mut Vec<PaneInfo>, now: Instant| -> Step {
            let step = waker.step(panes, false, now);
            if let Step::Wake(id, session) = &step {
                let i = panes.iter().position(|p| p.pane_id == *id).unwrap();
                panes[i] = running(*id, session);
                waker.started_one(*id, now);
                waker.record(*id, session.clone(), now);
                order.push(*id);
            }
            step
        };
        // The queue has not answered: the chiefs go, two at once, no clock between them.
        assert_eq!(turn(&mut waker, &mut panes, at(0)), Step::Wake(1, "pro".into()));
        assert_eq!(turn(&mut waker, &mut panes, at(0)), Step::Wake(2, "perso".into()));
        assert_eq!(turn(&mut waker, &mut panes, at(0)), Step::Wait, "the batch is full");
        waker.set_wanted(["night-qa", "lem-prd", "gone"].iter().map(|s| s.to_string()).collect());
        // The first chief shows its prompt: one slot, for a session with work.
        panes[0].ready = true;
        assert_eq!(turn(&mut waker, &mut panes, at(10)), Step::Wake(4, "night-qa".into()));
        assert_eq!(waker.take_ready(), [Ready { pane_id: 1, at: 10.0, took: 10.0, timed_out: false }]);
        // The other chief never shows one: its slot frees after the timeout.
        assert_eq!(turn(&mut waker, &mut panes, at(45)), Step::Wake(6, "lem-prd".into()));
        assert!(matches!(waker.take_ready()[..], [Ready { pane_id: 2, timed_out: true, .. }]));
        // Then the others, in tab order, as slots free.
        panes[3].ready = true;
        panes[5].ready = true;
        assert_eq!(turn(&mut waker, &mut panes, at(50)), Step::Wake(3, "idle-one".into()));
        assert_eq!(turn(&mut waker, &mut panes, at(50)), Step::Wake(5, "idle-two".into()));
        panes[2].ready = true;
        panes[4].ready = true;
        let done = turn(&mut waker, &mut panes, at(60));
        assert!(matches!(done, Step::Done { woken: 6, timed_out: 1, secs } if (59.0..=61.0).contains(&secs)), "{done:?}");
        assert_eq!(order, [1, 2, 4, 6, 3, 5]);
        assert!(panes.iter().all(|p| p.dormant.is_none()), "nobody is left asleep");
    }

    #[test]
    fn a_queue_that_does_not_answer_still_wakes_everyone_in_tab_order() {
        let t0 = Instant::now();
        let mut waker = LaunchWaker::selective(0.0, t0).paced(8, 45.0);
        let panes = vec![asleep(1, "worker", 2), asleep(2, "pro", 0)];
        assert_eq!(waker.step(&panes, false, t0), Step::Wake(2, "pro".into()));
        waker.started_one(2, t0);
        waker.record(2, "pro".into(), t0);
        assert_eq!(waker.step(&panes, false, t0), Step::Wait, "the queue may still answer: not the others yet");
        waker.set_wanted(HashSet::new());
        assert_eq!(waker.step(&panes, false, t0 + IDLE), Step::Wake(1, "worker".into()));
        assert_eq!(names(&[Some("✳ claap-agent"), None, Some(" ")]), ["claap-agent"]);
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
    fn a_conversation_woken_elsewhere_but_not_registered_is_not_woken_twice() {
        // Pane 2 was shown and resumes "a"; its agent has not registered yet.
        let panes = [asleep(1, "a", 2), running(2, "a"), asleep(3, "b", 2)];
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
    fn a_rank_is_the_one_first_seen_once_the_shell_retitles_the_pane() {
        let mut waker = LaunchWaker::new(3.0, Instant::now());
        let mut panes = vec![asleep(1, "a", 2), asleep(2, "pro", 0), asleep(3, "perso", 1)];
        assert_eq!(waker.pick(&panes), Some((2, "pro".into())));
        waker.record(2, "pro".into(), Instant::now());
        // The shell set its own title: the chief no longer reads as one.
        panes[2].rank = 2;
        assert_eq!(waker.pick(&panes), Some((3, "perso".into())));
    }

    #[test]
    fn an_absurd_stagger_falls_back_instead_of_panicking() {
        let start = Instant::now();
        for s in [f64::INFINITY, f64::NAN, 1e300, -1.0] {
            let mut waker = LaunchWaker::new(s, start);
            waker.record(1, "a".into(), start);
            assert!(waker.due(start + Duration::from_secs(3601)));
        }
    }

    #[test]
    fn an_empty_pick_looks_again_half_a_second_later() {
        let start = Instant::now();
        let mut waker = LaunchWaker::new(3.0, start);
        waker.idle(start);
        assert!(!waker.due(start + Duration::from_millis(499)));
        assert!(waker.due(start + IDLE));
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
