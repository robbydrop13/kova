//! What waits on the user in this pane, as cards in the chat: the pane's
//! `kova ask` questions (decision sheets and handoffs included) and the
//! permission prompt on its screen. A button answers through the path the
//! sidebar's Asks use: the board's answer queue, the side panel for a sheet,
//! `Pane::answer_permission` for a prompt.

use serde::Serialize;

use crate::ask::{Question, OTHER};
use crate::prompt_preview::{ParsedPrompt, PermissionChoice};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Question,
    Decision,
    Handoff,
    Permission,
}

/// What a button does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Answer the question with this button (a handoff step included: the
    /// board performs it and the question waits for `Fait`).
    Answer { action: String },
    /// Show the decision sheet in the side panel.
    Open,
    Permission { choice: Choice },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Approve,
    Always,
    Reject,
}

impl From<Choice> for PermissionChoice {
    fn from(c: Choice) -> Self {
        match c {
            Choice::Approve => PermissionChoice::Approve,
            Choice::Always => PermissionChoice::Always,
            Choice::Reject => PermissionChoice::Reject,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Button {
    pub label: String,
    #[serde(flatten)]
    pub act: Act,
    /// The free answer: needs text in the card's field.
    pub needs_text: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Card {
    /// The question's id, or `perm:<fingerprint>` for a prompt.
    pub id: String,
    pub kind: CardKind,
    pub from: String,
    pub message: String,
    /// A handoff's URL or command, a prompt's command or file.
    pub detail: Option<String>,
    /// The card shows a text field.
    pub input: bool,
    pub buttons: Vec<Button>,
    /// Said under the buttons when there is nothing to press here.
    pub hint: Option<String>,
}

/// The questions of the pane `pane_id` running `session` that the card would
/// show now, oldest first: asked from this pane, by this conversation when
/// both are known, and not snoozed.
pub fn for_pane<'a>(questions: impl IntoIterator<Item = &'a Question>, pane_id: u32, session: Option<&str>, now: u64) -> Vec<&'a Question> {
    questions
        .into_iter()
        .filter(|q| q.from.pane_id == Some(pane_id))
        .filter(|q| match (q.from.session_id.as_deref(), session) {
            (Some(asked), Some(running)) => asked == running,
            _ => true,
        })
        .filter(|q| q.due(now))
        .collect()
}

/// A question's card. `handoff_detail` is what the floating card shows under
/// a handoff (`handoff::Detail::card_text`), read by the caller since it
/// reads `handoff.toml`.
pub fn question_card(q: &Question, handoff_detail: Option<String>) -> Card {
    let answer = |label: &str| Button { label: label.to_string(), act: Act::Answer { action: label.to_string() }, needs_text: false };
    let (kind, buttons, input) = if q.is_decision() {
        let open = Button { label: "Open".to_string(), act: Act::Open, needs_text: false };
        (CardKind::Decision, if q.panel_link().is_some() { vec![open] } else { Vec::new() }, false)
    } else if crate::handoff::is_handoff(q) {
        (CardKind::Handoff, q.actions.iter().map(|a| answer(a)).collect(), false)
    } else {
        let buttons = q
            .buttons()
            .iter()
            .map(|b| Button { needs_text: b == OTHER || (q.actions.is_empty() && q.input), ..answer(b) })
            .collect();
        (CardKind::Question, buttons, q.input)
    };
    Card {
        id: q.id.clone(),
        kind,
        from: q.from.name.clone(),
        message: q.message.clone(),
        detail: handoff_detail.filter(|d| !d.is_empty()),
        input,
        hint: None,
        buttons,
    }
}

/// The id of a prompt's card: it changes with the prompt, so a click made
/// on one prompt never answers the next.
pub fn permission_id(prompt: &ParsedPrompt) -> String {
    format!("perm:{:016x}", prompt.fingerprint())
}

/// A permission prompt's card: Yes, the option that always allows, No,
/// whichever of the three the prompt numbers.
pub fn permission_card(prompt: &ParsedPrompt) -> Card {
    let choices = prompt.choices();
    let label_of = |n: Option<u32>| n.and_then(|n| prompt.options.iter().find(|(d, _)| *d == n)).map(|(_, l)| l.clone());
    let mut buttons = Vec::new();
    for (choice, digit) in [(Choice::Approve, choices.approve), (Choice::Always, choices.always), (Choice::Reject, choices.reject)] {
        if let Some(label) = label_of(digit) {
            let short = match choice {
                Choice::Approve => "Yes".to_string(),
                Choice::Reject => "No".to_string(),
                Choice::Always => label,
            };
            buttons.push(Button { label: short, act: Act::Permission { choice }, needs_text: false });
        }
    }
    let hint = buttons.is_empty().then(|| "Answer it in the terminal".to_string());
    Card {
        id: permission_id(prompt),
        kind: CardKind::Permission,
        from: prompt.header.clone(),
        message: prompt.question.clone(),
        detail: prompt.detail.clone(),
        input: false,
        buttons,
        hint,
    }
}

/// Whether `action` is a button this card offers: the page posts the label
/// it showed, and only those reach the board.
pub fn offers(card: &Card, action: &str) -> bool {
    card.buttons.iter().any(|b| matches!(&b.act, Act::Answer { action: a } if a == action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::Asker;

    fn question(id: &str, pane: Option<u32>, session: Option<&str>) -> Question {
        Question {
            id: id.into(),
            message: "Ship it?".into(),
            actions: vec!["Yes".into(), "No".into()],
            input: true,
            from: Asker { name: "kova".into(), pane_id: pane, session_id: session.map(Into::into), ..Default::default() },
            asked_at: 1,
            snoozed_until: 0,
            kind: None,
            url: None,
            run: None,
            sheet: None,
            draft: None,
            follow_up: false,
        }
    }

    #[test]
    fn only_this_pane_s_conversation_asks_here() {
        let mut snoozed = question("d", Some(4), None);
        snoozed.snoozed_until = 500;
        let qs = [
            question("a", Some(4), Some("s1")),
            question("b", Some(4), Some("old")),
            question("c", Some(5), Some("s1")),
            question("e", Some(4), None),
            snoozed,
        ];
        let ids: Vec<&str> = for_pane(&qs, 4, Some("s1"), 100).iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, ["a", "e"]);
        assert_eq!(for_pane(&qs, 4, Some("s1"), 600).len(), 3, "a snooze that ran out shows again");
    }

    #[test]
    fn a_plain_question_has_its_buttons_and_the_free_answer() {
        let card = question_card(&question("a", Some(4), None), None);
        assert_eq!(card.kind, CardKind::Question);
        let labels: Vec<(&str, bool)> = card.buttons.iter().map(|b| (b.label.as_str(), b.needs_text)).collect();
        assert_eq!(labels, [("Yes", false), ("No", false), ("Other", true)]);
        assert!(card.input);
        assert!(offers(&card, "No"));
        assert!(!offers(&card, "Maybe"));
    }

    #[test]
    fn a_question_without_actions_is_a_reply_field() {
        let mut q = question("a", Some(4), None);
        q.actions.clear();
        let card = question_card(&q, None);
        assert!(card.input);
        assert_eq!(card.buttons.len(), 1);
        assert!(card.buttons[0].needs_text);
        q.input = false;
        assert!(!question_card(&q, None).buttons[0].needs_text, "without a field, Reply is an acknowledgement");
    }

    #[test]
    fn a_decision_opens_its_sheet_and_a_handoff_offers_its_step() {
        let mut d = question("d", Some(4), None);
        d.kind = Some(crate::ask::KIND_DECISION.into());
        d.url = Some("https://claude.ai/artifact/x".into());
        let card = question_card(&d, None);
        assert_eq!(card.kind, CardKind::Decision);
        assert_eq!(card.buttons.iter().map(|b| b.act.clone()).collect::<Vec<_>>(), [Act::Open]);

        let mut h = question("h", Some(4), None);
        h.kind = Some(crate::handoff::KIND.into());
        h.url = Some("https://example.com/login".into());
        h.actions = vec![crate::handoff::OPEN.into(), crate::handoff::DONE.into()];
        h.input = false;
        let card = question_card(&h, Some("https://example.com/login".into()));
        assert_eq!(card.kind, CardKind::Handoff);
        assert_eq!(card.buttons.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["Ouvrir", "Fait"]);
        assert!(!card.input);
        assert_eq!(card.detail.as_deref(), Some("https://example.com/login"));
    }

    #[test]
    fn a_permission_card_offers_what_the_prompt_numbers() {
        let prompt = ParsedPrompt {
            header: "Bash command".into(),
            question: "Do you want to proceed?".into(),
            detail: Some("rm hello.txt".into()),
            lines: vec!["Bash command".into(), "rm hello.txt".into()],
            options: vec![(1, "Yes".into()), (2, "Yes, and don't ask again for rm commands".into()), (3, "No".into())],
        };
        let card = permission_card(&prompt);
        assert_eq!(card.id, permission_id(&prompt));
        let acts: Vec<(&str, Act)> = card.buttons.iter().map(|b| (b.label.as_str(), b.act.clone())).collect();
        assert_eq!(
            acts,
            [
                ("Yes", Act::Permission { choice: Choice::Approve }),
                ("Yes, and don't ask again for rm commands", Act::Permission { choice: Choice::Always }),
                ("No", Act::Permission { choice: Choice::Reject }),
            ]
        );
        let scrolled = ParsedPrompt { options: Vec::new(), ..prompt };
        let card = permission_card(&scrolled);
        assert!(card.buttons.is_empty());
        assert!(card.hint.is_some());
    }

    #[test]
    fn a_button_reaches_the_page_as_plain_json() {
        let b = Button { label: "No".into(), act: Act::Answer { action: "No".into() }, needs_text: false };
        assert_eq!(serde_json::to_value(&b).unwrap(), serde_json::json!({"label": "No", "act": "answer", "action": "No", "needs_text": false}));
    }
}
