//! What waits on the user in this pane, as cards in the chat: the pane's
//! `kova ask` questions (decision sheets and handoffs included) and the
//! permission prompt on its screen. A button answers through the path the
//! Cockpit uses: the board's answer queue, the side panel for a sheet,
//! `Pane::answer_permission` for a prompt.

use serde::Serialize;

use crate::ask::{Question, OTHER};
use crate::cockpit;
use crate::prompt_preview::Menu;
use crate::prompt_preview::{ParsedPrompt, PermissionChoice};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Question,
    Decision,
    Handoff,
    Permission,
    /// A menu read on screen (`prompt_preview::parse_menu`).
    Menu,
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

/// An ask's card on the Cockpit's markup (`cockpit::card_html_with`): its
/// first line as the title, what it wants, who asked and when, the rest of
/// the message and its answers opened under it, and the Cockpit's three
/// icons (snooze, remove, done). `linked` is the attention entry standing
/// for a decision sheet, as the Cockpit's opened card (detail and last
/// instruction filled by the caller): the chat shows that card, with its
/// instruction field, keyed by the ask.
pub fn ask_card_html(
    q: &Question,
    handoff_detail: Option<&str>,
    linked: Option<&cockpit::Item>,
    can_act: bool,
    scope: crate::attention::Scope,
    now: u64,
) -> String {
    let icons = [cockpit::Button::Snooze, cockpit::Button::Dismiss, cockpit::Button::Done];
    let sheet = if q.panel_link().is_some() { vec![cockpit::Button::OpenSheet] } else { Vec::new() };
    if let Some(entry) = linked.filter(|_| q.is_decision()) {
        let item = cockpit::Item { key: q.id.clone(), ask: Some(q.id.clone()), ..entry.clone() };
        return cockpit::card_html_with(&item, &sheet, &icons, can_act, now);
    }
    let (title, rest) = q.message.trim().split_once('\n').unwrap_or((q.message.trim(), ""));
    // The title is cut to one line: a long first line is said again in full.
    let mut message = if title.chars().count() > TITLE_MAX { q.message.trim() } else { rest.trim() }.to_string();
    if let Some(d) = handoff_detail.filter(|d| !d.trim().is_empty()) {
        message = if message.is_empty() { d.to_string() } else { format!("{message}\n{d}") };
    }
    let card = question_card(q, None);
    let (kind, needs) = match card.kind {
        CardKind::Decision => (cockpit::Kind::Sheet, "Répondre à la fiche"),
        CardKind::Handoff => (cockpit::Kind::Ask, "Une étape à faire de ton côté"),
        _ => (cockpit::Kind::Ask, "Répondre à la question"),
    };
    let answers: Vec<(String, bool)> = card
        .buttons
        .iter()
        .filter(|b| matches!(b.act, Act::Answer { .. }))
        .map(|b| (b.label.clone(), b.needs_text))
        .collect();
    let detail = (!message.is_empty() || !answers.is_empty() || card.input)
        .then(|| cockpit::Detail::Ask { message, answers, input: card.input });
    let item = cockpit::Item {
        key: q.id.clone(),
        kind,
        prio: 2,
        since: q.asked_at,
        title: title.to_string(),
        needs: needs.to_string(),
        from: q.from.name.clone(),
        scope,
        draft: None,
        openable: false,
        ask: None,
        detail,
        state: cockpit::State::Idle,
        instruction: None,
        shots: Vec::new(),
        label: None,
        validates: false,
        failed: None,
        uncertain: None,
        proof: None,
        plan: false,
        entry_kind: String::new(),
    };
    cockpit::card_html_with(&item, &sheet, &icons, true, now)
}

/// The id of a menu's card: it changes with the menu, so a click made on one
/// never answers the next.
pub fn menu_id(menu: &Menu) -> String {
    format!("menu:{:016x}", menu.fingerprint())
}

/// A menu Claude Code waits on (a permission prompt, an AskUserQuestion,
/// the plan approval), as a Cockpit card: what the frame says, the plan the
/// transcript holds, each option as a button, a field for text.
pub fn menu_card_html(menu: &Menu, plan: Option<&str>, now: u64) -> String {
    let (title, rest) = match menu.body.split_first() {
        Some((first, rest)) => (first.clone(), rest.to_vec()),
        None => ("Claude attend ta réponse".to_string(), Vec::new()),
    };
    let mut message: Vec<String> = rest;
    if let Some(plan) = plan {
        message.push(plan.trim().to_string());
    }
    for o in menu.options.iter().filter(|o| !o.free_text) {
        if let Some(d) = o.detail.as_deref().filter(|d| *d != o.label) {
            message.push(format!("{} : {}", o.label, d.replace('\n', " ")));
        }
    }
    let item = cockpit::Item {
        key: menu_id(menu),
        kind: cockpit::Kind::Ask,
        prio: 2,
        since: now,
        title,
        needs: menu.question.clone(),
        from: "Claude".to_string(),
        scope: crate::attention::Scope::Pro,
        draft: None,
        openable: false,
        ask: None,
        detail: Some(cockpit::Detail::Ask {
            message: message.join("\n"),
            answers: menu.options.iter().map(|o| (o.label.clone(), o.free_text)).collect(),
            input: true,
        }),
        state: cockpit::State::Idle,
        instruction: None,
        shots: Vec::new(),
        label: None,
        validates: false,
        failed: None,
        uncertain: None,
        proof: None,
        plan: false,
        entry_kind: String::new(),
    };
    cockpit::card_html_with(&item, &[], &[], false, now)
}

/// A menu's entry in the card list: its id, for the page and the clicks.
pub fn menu_card(menu: &Menu) -> Card {
    Card {
        id: menu_id(menu),
        kind: CardKind::Menu,
        from: "Claude".to_string(),
        message: menu.question.clone(),
        detail: None,
        input: true,
        buttons: Vec::new(),
        hint: None,
    }
}

/// The card shown when the session waits on something Kova cannot read
/// (a dialog of another shape): it says so, never guesses options.
pub fn unread_wait_html(now: u64) -> String {
    let item = cockpit::Item {
        key: "wait:unread".to_string(),
        kind: cockpit::Kind::Ask,
        prio: 2,
        since: now,
        title: "Claude attend ta réponse dans le terminal".to_string(),
        needs: "Kova ne sait pas lire cette question : réponds dans le terminal".to_string(),
        from: "Claude".to_string(),
        scope: crate::attention::Scope::Pro,
        draft: None,
        openable: false,
        ask: None,
        detail: None,
        state: cockpit::State::Idle,
        instruction: None,
        shots: Vec::new(),
        label: None,
        validates: false,
        failed: None,
        uncertain: None,
        proof: None,
        plan: false,
        entry_kind: String::new(),
    };
    cockpit::card_html_with(&item, &[], &[], false, now)
}

/// An answer to a menu: the option's digit, the text its field takes, and a
/// text that goes after as the next message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuAnswer {
    pub number: u32,
    pub text: Option<String>,
    pub after: Option<String>,
}

/// A click on the option `label` with `text` in the card's field. The
/// free-text option takes the text (one line: Enter would send it early);
/// any other option is answered and the text, if any, follows as a message.
pub fn menu_answer(menu: &Menu, label: &str, text: &str) -> Result<MenuAnswer, &'static str> {
    let option = menu.option(label).ok_or("Cette option n'est plus proposée")?;
    // One line (Enter would send it early), no control character, 8 KB at
    // most, as everything Kova types into a PTY.
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut one_line = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    while one_line.len() > 8 * 1024 {
        one_line.pop();
    }
    if option.free_text {
        if one_line.is_empty() {
            return Err("Écris ta réponse dans le champ");
        }
        return Ok(MenuAnswer { number: option.number, text: Some(one_line), after: None });
    }
    Ok(MenuAnswer { number: option.number, text: None, after: (!text.trim().is_empty()).then(|| text.to_string()) })
}

/// What the box sends while a menu with a free-text option waits: the
/// answer, at once, never queued.
pub fn box_answer(menu: &Menu, text: &str) -> Option<MenuAnswer> {
    let label = menu.free_text()?.label.clone();
    menu_answer(menu, &label, text).ok()
}

/// Where an answer under way stands, after its digit or its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuStage {
    Digit,
    Text,
}

/// What to write next, given the menu on screen now (`question` is the one
/// answered). After the digit: a menu gone means it took it; still there and
/// on that option, the text goes, or Enter. After the text: Enter, only
/// when the option shows it. `Err` says why nothing more goes.
pub fn menu_next(answer: &MenuAnswer, stage: MenuStage, question: &str, now: Option<&Menu>) -> Result<Option<String>, &'static str> {
    let Some(menu) = now.filter(|m| m.question == question) else {
        // Gone after a plain option's digit: it took it. A text still to
        // type has nowhere to go.
        return match (stage, &answer.text) {
            (MenuStage::Digit, None) => Ok(None),
            _ => Err("La réponse n'est pas partie : regarde le terminal"),
        };
    };
    if menu.selected != Some(answer.number) {
        return Err("Le menu a changé : regarde le terminal");
    }
    match (stage, &answer.text) {
        (MenuStage::Digit, Some(text)) => Ok(Some(text.clone())),
        (MenuStage::Digit, None) => Ok(Some("\r".to_string())),
        (MenuStage::Text, Some(text)) => {
            let shown = menu.options.iter().find(|o| o.number == answer.number).map(|o| o.label.as_str()).unwrap_or_default();
            let head: String = text.chars().take(20).collect();
            if shown.starts_with(&head) { Ok(Some("\r".to_string())) } else { Err("La réponse n'est pas partie : regarde le terminal") }
        }
        (MenuStage::Text, None) => Ok(None),
    }
}

/// The longest first line the card's title shows whole, roughly.
const TITLE_MAX: usize = 60;

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
    fn an_ask_is_a_cockpit_card_with_its_icons_in_french() {
        let mut d = question("430b8a55-1", Some(4), None);
        d.kind = Some(crate::ask::KIND_DECISION.into());
        d.url = Some("https://claude.ai/artifact/x".into());
        d.message = "Plan : 2 décisions\n\u{2022} Quand ?\n\u{2022} <Qui> ?".into();
        d.asked_at = 40;
        let html = ask_card_html(&d, None, None, true, crate::attention::Scope::Pro, 100);
        assert!(html.starts_with("<article class=\"card open\" data-key=\"430b8a55-1\">"), "{html}");
        assert!(html.contains("<div class=\"t\">Plan : 2 décisions</div>"));
        assert!(html.contains("Répondre à la fiche") && html.contains("kova \u{b7} il y a 1 min"), "{html}");
        assert!(html.contains("data-act=\"snooze\"") && html.contains("data-act=\"dismiss\"") && html.contains("data-act=\"done\""));
        assert!(html.contains("class=\"btn acc\" data-act=\"open\"><span>Décider</span>") && !html.contains("Ouvrir"), "{html}");
        assert!(html.contains("&lt;Qui&gt;") && !html.contains("<Qui>"), "escaped");
        assert!(!html.contains("cfrom") && !html.contains("class=\"ins\""), "no asker label, no instruction without an entry");

        // A plain question: its answers and its field, opened.
        let html = ask_card_html(&question("a", Some(4), None), None, None, true, crate::attention::Scope::Pro, 100);
        assert!(html.contains("data-act=\"answer\" data-arg=\"No\"") && html.contains("data-arg=\"Other\" data-text=\"1\""));
        assert!(html.contains("textarea class=\"ans\"") && html.contains("Répondre à la question"));

        // A sheet an attention entry stands for: the Cockpit's opened card, with
        // its instruction field, keyed by the ask.
        let entry = crate::cockpit::Item {
            key: "attention:a-1".into(),
            kind: crate::cockpit::Kind::Attention,
            prio: 2,
            since: 40,
            title: "Harnais commun".into(),
            needs: "Trancher".into(),
            from: "claap-agent".into(),
            scope: crate::attention::Scope::Pro,
            draft: None,
            openable: false,
            ask: None,
            detail: Some(crate::cockpit::Detail::Entry { needs: String::new(), body: "Le fond".into(), detail: String::new(), src: Vec::new(), links: Vec::new(), explaining: false }),
            state: crate::cockpit::State::Idle,
            instruction: None,
            shots: Vec::new(),
            label: None,
            validates: false,
        failed: None,
        uncertain: None,
        proof: None,
        plan: false,
            entry_kind: String::new(),
        };
        let html = ask_card_html(&d, None, Some(&entry), true, crate::attention::Scope::Pro, 100);
        assert!(html.contains("data-key=\"430b8a55-1\"") && html.contains("Harnais commun") && html.contains("Le fond"), "{html}");
        assert!(html.contains("<div class=\"lbl\">Consigne</div>") && html.contains("<span>Décider</span>") && html.contains("data-act=\"dismiss\""));
        assert!(!ask_card_html(&d, None, Some(&entry), false, crate::attention::Scope::Pro, 100).contains("Consigne"), "no act command, no field");
        let mut long = question("l", Some(4), None);
        long.message = "x".repeat(80);
        assert!(ask_card_html(&long, None, None, true, crate::attention::Scope::Pro, 100).contains(&format!("<p class=\"msg\">{}</p>", "x".repeat(80))));
    }

    fn screen(name: &str) -> Menu {
        let text = std::fs::read_to_string(format!("{}/tests/fixtures/prompt/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        crate::prompt_preview::parse_menu(&text).expect(name)
    }

    #[test]
    fn an_ask_user_question_answers_by_digit_or_by_its_typed_option() {
        let m = screen("ask-user-question.txt");
        let html = menu_card_html(&m, None, 100);
        assert!(html.contains(&format!("data-key=\"{}\"", menu_id(&m))));
        assert!(html.contains("Which color do you prefer?") && html.contains("data-arg=\"Green\""));
        assert!(html.contains("data-arg=\"Type something.\" data-text=\"1\"") && html.contains("textarea class=\"ans\""));
        assert!(!html.contains("Red : Red"), "a description that repeats its label is left out");

        // A plain option: its digit, nothing typed after.
        let a = menu_answer(&m, "Green", "").unwrap();
        assert_eq!(a, MenuAnswer { number: 2, text: None, after: None });
        assert_eq!(menu_next(&a, MenuStage::Digit, &m.question, None), Ok(None), "the menu took the digit and left");
        // The typed option: digit, then the text, then Enter once it shows.
        let a = box_answer(&m, "Purple\nplease").unwrap();
        assert_eq!(a, MenuAnswer { number: 4, text: Some("Purple please".into()), after: None });
        let mut on4 = m.clone();
        on4.selected = Some(4);
        assert_eq!(menu_next(&a, MenuStage::Digit, &m.question, Some(&on4)), Ok(Some("Purple please".into())));
        assert!(menu_next(&a, MenuStage::Text, &m.question, Some(&on4)).is_err(), "the option does not show the text yet");
        on4.options[3].label = "Purple please".into();
        assert_eq!(menu_next(&a, MenuStage::Text, &m.question, Some(&on4)), Ok(Some("\r".into())));
        assert!(menu_next(&a, MenuStage::Digit, &m.question, Some(&m)).is_err(), "the cursor is elsewhere: nothing goes");
        assert_eq!(menu_answer(&m, "Type something.", " "), Err("Écris ta réponse dans le champ"));
        assert_eq!(menu_answer(&m, "Type something.", "a\u{1b}[201~b\u{15}").unwrap().text.as_deref(), Some("a [201~b"));
        assert!(menu_next(&a, MenuStage::Digit, &m.question, None).is_err(), "a text with nowhere to go is not lost silently");
        assert!(menu_answer(&m, "Yellow", "").is_err());
    }

    #[test]
    fn a_plan_card_shows_the_plan_and_a_permission_answer_lets_the_text_follow() {
        let m = screen("plan-approval.txt");
        let html = menu_card_html(&m, Some("1. Create c.txt\n2. Check it"), 100);
        assert!(html.contains("1. Create c.txt\n2. Check it") && html.contains("data-arg=\"Yes, manually approve edits\""));
        assert_eq!(box_answer(&m, "Name it d.txt").map(|a| a.number), Some(3));

        let m = screen("prompt-write-wrapped.txt");
        assert!(box_answer(&m, "x").is_none(), "no typed option: the box queues as before");
        let a = menu_answer(&m, "No", "Write it in /tmp instead").unwrap();
        assert_eq!(a, MenuAnswer { number: 3, text: None, after: Some("Write it in /tmp instead".into()) });
        // Still on the menu, cursor on 3 after the digit: Enter confirms.
        let mut on3 = m.clone();
        on3.selected = Some(3);
        assert_eq!(menu_next(&a, MenuStage::Digit, &m.question, Some(&on3)), Ok(Some("\r".into())));
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
