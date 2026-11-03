//! What waits on the user, in the order `Cmd+J` walks it: a turn that died on
//! a provider error, a scheduled routine whose last run failed, a permission
//! prompt, an open `kova ask`. Pure: `sidebar_ui::sync_needs` does the live
//! reads once per tick, and this decides what counts, in which order, and what
//! the sidebar badges say, so the key and the icons cannot drift apart.
//!
//! `waits_on_you` is narrower than `need_items` on purpose. It counts only what
//! a human has to answer, a permission prompt and an open question: those hold
//! an agent that is already sitting at its prompt. An error and a failed
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
}

/// One thing that wants the user, as the sidebar and Next see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedItem {
    pub kind: NeedKind,
    /// Stable across ticks: `pane:12`, `routine:<name>`, `ask:<id>`.
    pub key: String,
    /// The pane to land on when there is one; a failed routine has none.
    pub pane_id: Option<u64>,
    /// Epoch seconds since when it waits: the oldest is the most urgent.
    pub since: u64,
    /// What the row says: the pane's title, the routine's name, the question.
    pub title: String,
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
/// failed routines oldest first, then permission prompts and open questions
/// merged by age.
///
/// A pane holding a permission prompt and an open question is one thing to
/// deal with, not two: only the oldest of the two is kept, and the ask wins a
/// tie. A pane with an error and an ask keeps both: two different problems,
/// one ends the turn and the other blocks it.
pub fn need_items(input: &NeedInput) -> Vec<NeedItem> {
    let mut errors: Vec<NeedItem> = pane_items(&input.panes, NeedKind::Error, |pane| {
        pane.error_since
    });
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
            })
        })
        .collect();
    routines.sort_by(by_age);
    errors.sort_by(by_age);

    let asks: Vec<NeedItem> = input
        .asks
        .iter()
        .filter(|ask| open(ask, input.now))
        .map(|ask| NeedItem {
            kind: NeedKind::Ask,
            key: format!("ask:{}", ask.id),
            pane_id: ask.pane_id,
            since: ask.created_at,
            title: ask.title.clone(),
        })
        .collect();
    let permissions = pane_items(&input.panes, NeedKind::Permission, |pane| {
        pane.permission_since
    });

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

    let mut merged: Vec<NeedItem> = permissions
        .into_iter()
        .zip(keep_permission)
        .filter(|(_, keep)| *keep)
        .map(|(item, _)| item)
        .chain(asks.into_iter().zip(keep_ask).filter(|(_, keep)| *keep).map(|(item, _)| item))
        .collect();
    merged.sort_by(by_age);

    errors.into_iter().chain(routines).chain(merged).collect()
}

/// The pane items of one kind, in no particular order.
fn pane_items(
    panes: &[PaneNeed],
    kind: NeedKind,
    since: impl Fn(&PaneNeed) -> Option<u64>,
) -> Vec<NeedItem> {
    panes
        .iter()
        .filter_map(|pane| {
            Some(NeedItem {
                kind,
                key: format!("pane:{}", pane.id),
                pane_id: Some(pane.id),
                since: since(pane)?,
                title: pane.title.clone(),
            })
        })
        .collect()
}

/// What a human has to answer right now, across the app: permission prompts
/// and open questions, counted the way `need_items` dedupes them. Errors and
/// failed runs are not in it: they want a look, not an answer.
pub fn waits_on_you(input: &NeedInput) -> u32 {
    need_items(input)
        .iter()
        .filter(|item| matches!(item.kind, NeedKind::Permission | NeedKind::Ask))
        .count() as u32
}

/// The pane Next lands on: the need pane after the one under the eye, in
/// need order, wrapping, so repeated presses visit every waiting pane. From
/// anywhere else, the first one. A failed routine carries no pane, so the
/// walk steps over it here. `None` when the only need pane is the current one.
pub fn next_need_pane(items: &[NeedItem], current: Option<u64>) -> Option<u64> {
    let mut panes: Vec<u64> = Vec::new();
    for id in items.iter().filter_map(|item| item.pane_id) {
        if !panes.contains(&id) {
            panes.push(id);
        }
    }
    let start = match current.and_then(|c| panes.iter().position(|id| *id == c)) {
        Some(at) => at + 1,
        None => 0,
    };
    (0..panes.len())
        .map(|step| panes[(start + step) % panes.len()])
        .find(|id| Some(*id) != current)
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
            let keys: Vec<String> =
                need_items(&case.input).into_iter().map(|item| item.key).collect();
            assert_eq!(keys, case.expect.order, "{}: order", case.name);
            assert_eq!(
                waits_on_you(&case.input),
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
                PaneNeed { id: 3, error_since: Some(10), permission_since: None, title: "a".into() },
                PaneNeed { id: 4, error_since: None, permission_since: Some(20), title: "b".into() },
            ],
            routines: vec![RoutineNeed {
                name: "nightly".into(),
                failed_at: Some(5),
                paused: false,
            }],
            asks: Vec::new(),
            now: 100,
        };
        let items = need_items(&input);
        assert_eq!(first_failed_routine(&items), Some("nightly"));
        assert_eq!(next_need_pane(&items, None), Some(3));
        // Standing on one, the other comes up rather than itself.
        assert_eq!(next_need_pane(&items, Some(3)), Some(4));
        assert_eq!(next_need_pane(&items, Some(4)), Some(3));
        assert_eq!(next_need_pane(&[], None), None);
    }

    #[test]
    fn next_visits_every_need_pane_in_turn() {
        let item = |pane: u64| NeedItem {
            kind: NeedKind::Ask,
            key: format!("ask:{pane}"),
            pane_id: Some(pane),
            since: pane,
            title: String::new(),
        };
        let items = vec![item(1), item(2), item(3)];
        assert_eq!(next_need_pane(&items, Some(1)), Some(2));
        assert_eq!(next_need_pane(&items, Some(2)), Some(3));
        assert_eq!(next_need_pane(&items, Some(3)), Some(1));
        assert_eq!(next_need_pane(&items, Some(9)), Some(1));
        assert_eq!(next_need_pane(&items[..1], Some(1)), None);
    }
}
