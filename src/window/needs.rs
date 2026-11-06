//! What waits on the user, in the order `Cmd+J` walks it: a turn that died on
//! a provider error, a scheduled routine whose last run failed, a permission
//! prompt, an open `kova ask`, an attention entry that wants the user now
//! (`[attention]` in the config, `crate::attention`). Pure: `sidebar_ui::sync_needs` does the live
//! reads once per tick, and this decides what counts, in which order, and what
//! the sidebar badges say, so the key and the icons cannot drift apart.
//!
//! `waits_on_you` is narrower than `need_items` on purpose. It counts only what
//! a human has to answer, a permission prompt, an open question and a `now`
//! attention entry: the first two hold an agent that is already sitting at
//! its prompt, the last one holds someone the queue names. An error and a failed
//! routine are need items so Next walks them, but they are not waiting on an
//! answer, the turn or the run is simply over.
//!
//! `tests/fixtures/waits_on_you.json` is the shared statement of this order:
//! KovaLink copies it verbatim, so it stays self-contained and made up.

use serde::Deserialize;

/// What kind of problem an item is. Declaration order is the tie-break at an
/// equal `since`, and the order the groups are laid out in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NeedKind {
    /// The last turn died on a provider error: an expired login, a limit.
    Error,
    /// A scheduled routine's last run exited non-zero.
    FailedRoutine,
    /// A permission prompt sits on screen, nobody answered it.
    Permission,
    /// An agent's `kova ask` is open.
    Ask,
    /// An entry of the attention queue at level `now`: no pane, so Next
    /// steps over it, but it is something to answer.
    Attention,
}

/// One thing that wants the user, as the sidebar and Next see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedItem {
    pub kind: NeedKind,
    /// Stable across ticks: `pane:12`, `routine:<name>`, `ask:<id>`,
    /// `attention:<id>`.
    pub key: String,
    /// The pane to land on when there is one; a failed routine has none.
    pub pane_id: Option<u64>,
    /// Epoch seconds since when it waits: the oldest is the most urgent.
    pub since: u64,
    /// Who the row names: the pane's title for an error, the routine's name
    /// for a failed run, the asker (agent or session) for an ask or a
    /// permission. A question with no asker falls back to its own text.
    pub title: String,
    /// The row's detail column: the provider's words for an error, the
    /// command a permission wants, the question. Empty when there is none.
    pub detail: String,
}

/// What a pane is waiting on, if anything.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneNeed {
    pub id: u64,
    #[serde(default)]
    pub error_since: Option<u64>,
    #[serde(default)]
    pub permission_since: Option<u64>,
    pub title: String,
    /// Who is asking, for a permission prompt: the pane's agent session.
    /// Absent on older reports and in the shared fixture, where the pane's
    /// own title stands in.
    #[serde(default)]
    pub from: Option<String>,
    /// The error's text, or the permission prompt's detail (the command, the
    /// file). Absent on older reports and in the shared fixture.
    #[serde(default)]
    pub detail: Option<String>,
}

/// A scheduled routine's last run, if it failed.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineNeed {
    pub name: String,
    #[serde(default)]
    pub failed_at: Option<u64>,
    /// Switched off on purpose: a failure from before does not count.
    #[serde(default)]
    pub paused: bool,
}

/// A `kova ask` question, as the board reports it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskNeed {
    pub id: String,
    #[serde(default)]
    pub pane_id: Option<u64>,
    pub created_at: u64,
    /// `None` when it was never put off; an ask snoozed to or past `now` is back.
    #[serde(default)]
    pub snoozed_until: Option<u64>,
    #[serde(default)]
    pub answered: bool,
    pub title: String,
    /// Who asked (`Asker::name`: the agent or the session), shown as the
    /// row's title. Absent in the shared fixture.
    #[serde(default)]
    pub from: Option<String>,
    /// What the question says after its first line.
    #[serde(default)]
    pub detail: Option<String>,
}

/// An entry of the attention queue. Only level `now` is a need: `point` and
/// `memo` wait for a later look, the Cockpit lists `point` on its own.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionNeed {
    pub id: String,
    /// Epoch seconds the entry was added.
    pub since: u64,
    pub level: String,
    pub title: String,
}

/// Everything the model needs to know, at one clock reading.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeedInput {
    #[serde(default)]
    pub panes: Vec<PaneNeed>,
    #[serde(default)]
    pub routines: Vec<RoutineNeed>,
    #[serde(default)]
    pub asks: Vec<AskNeed>,
    #[serde(default)]
    pub attention: Vec<AttentionNeed>,
    pub now: u64,
}

/// Whether an ask is still waiting: unanswered, and not put off past `now`.
/// A snooze that has run out brings it back where it was.
fn open(ask: &AskNeed, now: u64) -> bool {
    !ask.answered && ask.snoozed_until.is_none_or(|until| until <= now)
}

/// Oldest first, then by kind, then by key: a total order, so the list a tick
/// produces is always the same one and the sidebar only redraws on a change.
fn by_age(a: &NeedItem, b: &NeedItem) -> std::cmp::Ordering {
    (a.since, a.kind).cmp(&(b.since, b.kind)).then_with(|| a.key.cmp(&b.key))
}

/// Every need item, in the order `Next` walks them: errors oldest first, then
/// failed routines oldest first, then permission prompts, open questions and
/// `now` attention entries merged by age.
///
/// A pane holding a permission prompt and an open question is one thing to
/// deal with, not two: only the oldest of the two is kept, and the ask wins a
/// tie. A pane with an error and an ask keeps both: two different problems,
/// one ends the turn and the other blocks it.
pub fn need_items(input: &NeedInput) -> Vec<NeedItem> {
    let mut errors: Vec<NeedItem> = pane_items(
        &input.panes,
        NeedKind::Error,
        |pane| pane.error_since,
        |pane| pane.title.clone(),
    );
    let mut routines: Vec<NeedItem> = input
        .routines
        .iter()
        .filter(|routine| !routine.paused)
        .filter_map(|routine| {
            let since = routine.failed_at?;
            Some(NeedItem {
                kind: NeedKind::FailedRoutine,
                key: format!("routine:{}", routine.name),
                pane_id: None,
                since,
                title: routine.name.clone(),
                detail: "Routine failed".to_string(),
            })
        })
        .collect();
    routines.sort_by(by_age);
    errors.sort_by(by_age);

    let asks: Vec<NeedItem> = input
        .asks
        .iter()
        .filter(|ask| open(ask, input.now))
        .map(|ask| {
            // The row names the asker; the question is its detail column.
            let question = match ask.detail.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                Some(rest) => format!("{} {}", ask.title.trim(), rest),
                None => ask.title.clone(),
            };
            let from = ask.from.clone().filter(|from| !from.trim().is_empty());
            NeedItem {
                kind: NeedKind::Ask,
                detail: question.clone(),
                key: format!("ask:{}", ask.id),
                pane_id: ask.pane_id,
                since: ask.created_at,
                title: from.unwrap_or(question),
            }
        })
        .collect();
    let permissions = pane_items(
        &input.panes,
        NeedKind::Permission,
        |pane| pane.permission_since,
        |pane| pane.from.clone().filter(|from| !from.trim().is_empty()).unwrap_or_else(|| pane.title.clone()),
    );

    // One pane, one thing: its oldest open question and its permission prompt
    // collapse into whichever appeared first.
    let mut keep_ask = vec![true; asks.len()];
    let mut keep_permission = vec![true; permissions.len()];
    for (pi, permission) in permissions.iter().enumerate() {
        let Some(pane_id) = permission.pane_id else { continue };
        let oldest = asks
            .iter()
            .enumerate()
            .filter(|(ai, ask)| keep_ask[*ai] && ask.pane_id == Some(pane_id))
            .min_by_key(|(_, ask)| ask.since);
        let Some((ai, ask)) = oldest else { continue };
        if ask.since <= permission.since {
            keep_permission[pi] = false;
        } else {
            keep_ask[ai] = false;
        }
    }

    let attention = input.attention.iter().filter(|entry| entry.level == "now").map(|entry| NeedItem {
        kind: NeedKind::Attention,
        key: format!("attention:{}", entry.id),
        pane_id: None,
        since: entry.since,
        title: entry.title.clone(),
        detail: String::new(),
    });
    let mut merged: Vec<NeedItem> = permissions
        .into_iter()
        .zip(keep_permission)
        .filter(|(_, keep)| *keep)
        .map(|(item, _)| item)
        .chain(asks.into_iter().zip(keep_ask).filter(|(_, keep)| *keep).map(|(item, _)| item))
        .chain(attention)
        .collect();
    merged.sort_by(by_age);

    errors.into_iter().chain(routines).chain(merged).collect()
}

/// The pane items of one kind, in no particular order. `title` is what the
/// row names: the pane's own title, or the asker for a permission prompt.
fn pane_items(
    panes: &[PaneNeed],
    kind: NeedKind,
    since: impl Fn(&PaneNeed) -> Option<u64>,
    title: impl Fn(&PaneNeed) -> String,
) -> Vec<NeedItem> {
    panes
        .iter()
        .filter_map(|pane| {
            Some(NeedItem {
                kind,
                detail: pane.detail.clone().unwrap_or_default(),
                key: format!("pane:{}", pane.id),
                pane_id: Some(pane.id),
                since: since(pane)?,
                title: title(pane),
            })
        })
        .collect()
}

/// What a human has to answer right now, across the app: permission prompts,
/// open questions and `now` attention entries, counted from the list the tick already built (asking
/// `need_items` a second time per window was pure waste). Errors and failed
/// runs are not in it: they want a look, not an answer.
pub fn waits_on_you(items: &[NeedItem]) -> u32 {
    items
        .iter()
        .filter(|item| matches!(item.kind, NeedKind::Permission | NeedKind::Ask | NeedKind::Attention))
        .count() as u32
}

/// The need panes `Cmd+J` has already landed on since the walk started: each
/// is visited once, then the key moves on to unread output. The walk resets
/// here when it reaches the end, and a need list that changed (another set of
/// keys) resets it too, so a prompt that just appeared is visited at once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NeedWalk {
    /// The keys of the list the walk started on.
    keys: Vec<String>,
    /// The need panes it already landed on.
    seen: Vec<u64>,
}

impl NeedWalk {
    /// A walk that has not started, for the static that holds the running one.
    pub const fn new() -> Self {
        Self { keys: Vec::new(), seen: Vec::new() }
    }

    /// The need pane to visit on this press, in list order; `None` once every
    /// one has been visited, which is what lets Cmd+J fall through to the
    /// unread and idle tiers. That `None` also starts a new walk, so the next
    /// press goes back to the first pane. A failed routine has no pane and is
    /// stepped over.
    pub fn next(&mut self, items: &[NeedItem], current: Option<u64>) -> Option<u64> {
        let keys: Vec<String> = items.iter().map(|item| item.key.clone()).collect();
        if keys != self.keys {
            self.keys = keys;
            self.seen.clear();
        }
        let mut panes: Vec<u64> = Vec::new();
        for id in items.iter().filter_map(|item| item.pane_id) {
            if !panes.contains(&id) {
                panes.push(id);
            }
        }
        // From the pane under the eye forward, wrapping: the next need pane in
        // the list, never the one just left, and never one already visited.
        let start = match current.and_then(|c| panes.iter().position(|id| *id == c)) {
            Some(at) => at + 1,
            None => 0,
        };
        let next = (0..panes.len())
            .map(|step| panes[(start + step) % panes.len()])
            .find(|id| Some(*id) != current && !self.seen.contains(id));
        match next {
            Some(id) => {
                self.seen.push(id);
                Some(id)
            }
            None => {
                self.seen.clear();
                None
            }
        }
    }
}

/// The oldest failed routine, for the sidebar's Scheduled list: the list is
/// the only place a failure lives, there is no pane to focus.
pub fn first_failed_routine(items: &[NeedItem]) -> Option<&str> {
    items.iter().find(|item| item.kind == NeedKind::FailedRoutine).map(|item| item.title.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One case of `tests/fixtures/waits_on_you.json`.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Case {
        name: String,
        input: NeedInput,
        expect: Expect,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Expect {
        waits_on_you: u32,
        order: Vec<String>,
    }

    #[test]
    fn every_shared_case_holds() {
        let text =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/waits_on_you.json"));
        let cases: Vec<Case> =
            serde_json::from_str(text).expect("tests/fixtures/waits_on_you.json is a case array");
        // The file is what KovaLink copies: a thin sample would let the two
        // sides drift on the cases nobody wrote down.
        assert!(cases.len() >= 8, "the shared fixture keeps its cases");
        for case in cases {
            // One list, read once: the order and the badge both come from it.
            let items = need_items(&case.input);
            let keys: Vec<String> = items.iter().map(|item| item.key.clone()).collect();
            assert_eq!(keys, case.expect.order, "{}: order", case.name);
            assert_eq!(
                waits_on_you(&items),
                case.expect.waits_on_you,
                "{}: waits_on_you",
                case.name
            );
        }
    }

    #[test]
    fn next_steps_over_failed_routines_and_the_pane_under_the_eye() {
        let input = NeedInput {
            panes: vec![
                PaneNeed { id: 3, error_since: Some(10), permission_since: None, title: "a".into(), from: None, detail: None },
                PaneNeed { id: 4, error_since: None, permission_since: Some(20), title: "b".into(), from: None, detail: None },
            ],
            routines: vec![RoutineNeed {
                name: "nightly".into(),
                failed_at: Some(5),
                paused: false,
            }],
            asks: Vec::new(),
            attention: Vec::new(),
            now: 100,
        };
        let items = need_items(&input);
        assert_eq!(first_failed_routine(&items), Some("nightly"));
        let mut walk = NeedWalk::new();
        assert_eq!(walk.next(&items, None), Some(3));
        // Standing on one, the other comes up rather than itself.
        assert_eq!(walk.next(&items, Some(3)), Some(4));
        // Both had their turn: the walk ends (unread output gets the key) and
        // the press after that starts over.
        assert_eq!(walk.next(&items, Some(4)), None);
        assert_eq!(walk.next(&items, Some(4)), Some(3));
        assert_eq!(NeedWalk::new().next(&[], None), None);
    }

    #[test]
    fn next_visits_every_need_pane_once_then_starts_over() {
        let item = |pane: u64| NeedItem {
            kind: NeedKind::Ask,
            key: format!("ask:{pane}"),
            pane_id: Some(pane),
            since: pane,
            title: String::new(),
            detail: String::new(),
        };
        let items = vec![item(1), item(2), item(3)];
        let mut walk = NeedWalk::new();
        assert_eq!(walk.next(&items, Some(1)), Some(2));
        assert_eq!(walk.next(&items, Some(2)), Some(3));
        // 1 was where the walk started, so it is the last one to get a turn;
        // only then does the tier end and give the key to unread output.
        assert_eq!(walk.next(&items, Some(3)), Some(1));
        assert_eq!(walk.next(&items, Some(1)), None, "every need pane was visited once");
        assert_eq!(walk.next(&items, Some(1)), Some(2), "a new walk starts where the eye is");

        // Another list (a fourth pane asks): the walk restarts instead of
        // resuming in the middle of the old one.
        let mut walk = NeedWalk::new();
        assert_eq!(walk.next(&items, None), Some(1));
        assert_eq!(walk.next(&items, Some(1)), Some(2));
        let mut grown = items.clone();
        grown.push(item(4));
        assert_eq!(walk.next(&grown, Some(1)), Some(2));
    }

    #[test]
    fn a_need_row_names_the_asker_and_puts_the_question_under_it() {
        let input = NeedInput {
            panes: vec![
                PaneNeed { id: 3, error_since: Some(10), permission_since: None, title: "api".into(), from: None, detail: Some("session limit".into()) },
                PaneNeed { id: 4, error_since: None, permission_since: Some(20), title: "Pane 4".into(), from: Some("Acme".into()), detail: Some("rm -rf build".into()) },
            ],
            routines: vec![RoutineNeed { name: "nightly".into(), failed_at: Some(5), paused: false }],
            asks: vec![AskNeed {
                id: "a1".into(),
                pane_id: Some(7),
                created_at: 30,
                snoozed_until: None,
                answered: false,
                title: "Ship the draft?".into(),
                from: Some("claude".into()),
                detail: Some("The copy is ready.".into()),
            }],
            attention: Vec::new(),
            now: 100,
        };
        let items = need_items(&input);
        let row = |kind: NeedKind, pane: Option<u64>| {
            items
                .iter()
                .find(|item| item.kind == kind && item.pane_id == pane)
                .map(|item| (item.title.as_str(), item.detail.as_str()))
                .expect("the row exists")
        };
        // An error keeps the pane's name and says what the provider said.
        assert_eq!(row(NeedKind::Error, Some(3)), ("api", "session limit"));
        // A permission names the asker, the pane only when there is none.
        assert_eq!(row(NeedKind::Permission, Some(4)), ("Acme", "rm -rf build"));
        // A failed routine names itself and says the run failed.
        assert_eq!(row(NeedKind::FailedRoutine, None), ("nightly", "Routine failed"));
        // An ask names the agent, its question below.
        assert_eq!(row(NeedKind::Ask, Some(7)), ("claude", "Ship the draft? The copy is ready."));
    }
}
