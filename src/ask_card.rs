//! The card an agent's question appears on (`crate::ask`).
//!
//! A port of an earlier Swift prototype's `QuestionCard.swift`. Its structure
//! stays (header, message, field, buttons, one card at a time); its look moved
//! to the Claap design system on 04/11/2026 at the user's request, the same
//! tokens and type as the sidebar's Asks cards (`sidebar::tokens`, Inter): a
//! `bg1` card with a `b3` rim and a 12 pt radius, the asker's colour on a dot,
//! the primary answer on the accent fill, the others secondary.
//!
//! A decision question (`kova ask --decision <url>`) says `New decision` and
//! adds an Open button that shows the sheet in the side panel without
//! answering; the agent hears back when the user clicks Done.
//!
//! The card sits top-right of the screen, over everything, without taking the
//! keyboard until the user clicks into it. One question at a time; the others
//! wait behind it ("N more").
//!
//! Two things were added here afterwards, for this fork only: Cmd+C, which
//! copies what the mouse selected in the message (the card often shows a path
//! or a command worth keeping, the body has always been selectable, and
//! nothing in Kova turned that key into a copy), and a "Later" control that
//! puts the question off for a chosen delay. No copy button: the gesture is
//! select and Cmd+C, as anywhere else. The rest of the look is Kova Float's
//! and stays as it is.
//!
//! Layout: a borderless rounded panel. Header (the asker's dot, who asks, time,
//! how many wait), the message, the answer field, then the buttons
//! right-aligned with the first one as the default (accent, Enter).
//!
//! Main thread only. The answers are queued here and drained by Kova's render
//! tick through [`take_answers`]: AppKit calls the button and field callbacks
//! from inside its event loop, where the window list may be borrowed.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSAttributedStringNSExtendedStringDrawing, NSAttributedStringNSStringDrawing,
    NSBackingStoreType, NSButton, NSColor, NSControl, NSControlTextEditingDelegate,
    NSEvent, NSEventModifierFlags, NSFloatingWindowLevel, NSFocusRingType, NSFont,
    NSFontAttributeName,
    NSForegroundColorAttributeName, NSLayoutAttribute, NSLayoutConstraint,
    NSMenu, NSMenuItem,
    NSLayoutConstraintOrientation, NSLayoutManager, NSLayoutPriorityRequired, NSLineBreakMode,
    NSPanel, NSRequestUserAttentionType, NSResponder, NSScreen, NSSound, NSStackView,
    NSStringDrawingOptions, NSTextField, NSTextFieldDelegate, NSTextView,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSDate,
    NSDateFormatter, NSDateFormatterStyle, NSDictionary, NSEdgeInsets, NSNotification, NSObject,
    NSObjectProtocol, NSString,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Mutex;

/// One question as the card shows it.
pub struct CardQuestion<'a> {
    pub id: &'a str,
    /// The agent's name, shown on the card and hashed into its colour.
    pub from: &'a str,
    pub message: &'a str,
    /// Never empty, in order; the first is the primary (drawn last, rightmost).
    pub buttons: &'a [String],
    /// Whether to show the answer field.
    pub input: bool,
    /// Epoch seconds; shown as the local short time.
    pub asked_at: u64,
    /// A decision sheet's URL: the card says `New decision` and offers Open.
    pub decision_url: Option<&'a str>,
}

/// What the user answered: the button (or the first one, for Enter) and the
/// field's text, trimmed.
pub struct CardAnswer {
    pub id: String,
    pub action: String,
    pub text: String,
}

/// A question the user put off, and for how long.
pub struct CardSnooze {
    pub id: String,
    pub seconds: u64,
}

/// The card's width, as in Swift.
const WIDTH: CGFloat = 400.0;
/// Horizontal inset of every row; the vertical one is 4 points less.
const INSET: CGFloat = 20.0;
/// Beyond this the field stops growing and scrolls.
const MAX_LINES: CGFloat = 8.0;
const FIELD_FONT_SIZE: CGFloat = 13.0;
/// The design system's card radius (`xl`), button and input radius (`lg`).
const CARD_RADIUS: CGFloat = 12.0;
const CONTROL_RADIUS: CGFloat = 8.0;
/// The asker's dot, as the sidebar's awaiting dot.
const DOT_D: CGFloat = 8.0;
const BUTTON_H: CGFloat = 32.0;
const PLACEHOLDER: &str = "Type an answer, or just click a button";

/// Answers given since the last [`take_answers`]. A `Mutex` although only the
/// main thread touches it: `take_answers` takes no `MainThreadMarker`, so a
/// thread_local would silently hand another thread an empty queue.
static ANSWERS: Mutex<Vec<CardAnswer>> = Mutex::new(Vec::new());
/// Snoozes chosen since the last [`take_snoozes`], same reasoning as `ANSWERS`.
static SNOOZES: Mutex<Vec<CardSnooze>> = Mutex::new(Vec::new());
/// Decision sheets the user asked to open, since the last [`take_panel_opens`].
static PANEL_OPENS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

thread_local! {
    /// The card, built on the first `show`. Kept for the life of the process:
    /// the panel is ordered out, never closed.
    static CARD: RefCell<Option<Rc<Card>>> = const { RefCell::new(None) };
    /// The count last written to the Dock badge: `show` may run every tick, and
    /// the badge is only touched when the count changes.
    static BADGE: Cell<Option<usize>> = const { Cell::new(None) };
}

define_class!(
    /// Swift's `FloatPanel`: a non-activating panel that still accepts the
    /// keyboard when clicked. A borderless window answers NO to
    /// `canBecomeKeyWindow` by default, so without this the field could never
    /// be typed into.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaAskCardPanel"]
    struct CardPanel;

    impl CardPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// Cmd+C copies what the mouse selected (in the message or in the answer
        /// being typed), and the whole message when nothing is selected. Kova
        /// has no Edit menu, so nothing else in the app turns Cmd+C into a copy:
        /// the panel has to do it. The body has always been selectable, so this
        /// is the whole of the gesture, and there is no copy button.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            if is_copy_key(event) {
                if let Some(card) = card() {
                    card.copy();
                    return Bool::YES;
                }
            }
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }
    }
);

define_class!(
    /// Target of the buttons and delegate of the answer field. Holds nothing:
    /// the state lives in `CARD`, read when a callback fires.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaAskCardTarget"]
    struct CardTarget;

    unsafe impl NSObjectProtocol for CardTarget {}

    unsafe impl NSControlTextEditingDelegate for CardTarget {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            if let Some(card) = card() {
                card.fit_field();
            }
        }

        /// Enter in the field sends with "Other" when text was typed and the
        /// card has it, else with the first button; Shift+Enter (or
        /// Option+Enter) adds a line.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_text_view_do_command(
            &self,
            _control: &NSControl,
            text_view: &NSTextView,
            selector: Sel,
        ) -> bool {
            enter_in_field(MainThreadMarker::from(self), text_view, selector)
        }
    }

    unsafe impl NSTextFieldDelegate for CardTarget {}

    impl CardTarget {
        /// The "Later" button: the delays, as a menu under it.
        #[unsafe(method(showSnoozeMenu:))]
        fn show_snooze_menu(&self, sender: &NSButton) {
            let mtm = MainThreadMarker::from(self);
            let menu = NSMenu::new(mtm);
            for (index, (label, _)) in crate::ask::SNOOZE_CHOICES.iter().enumerate() {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&ns(label));
                item.setTag(index as isize);
                unsafe {
                    item.setTarget(Some(self));
                    item.setAction(Some(sel!(snoozed:)));
                }
                menu.addItem(&item);
            }
            let view: &NSView = sender.as_ref();
            // Negative y: just under the button, in an unflipped view.
            menu.popUpMenuPositioningItem_atLocation_inView(None, CGPoint::new(0.0, -4.0), Some(view));
        }

        /// A delay was picked: its tag is its index in `ask::SNOOZE_CHOICES`.
        #[unsafe(method(snoozed:))]
        fn snoozed(&self, sender: &NSMenuItem) {
            let Some(card) = card() else { return };
            let chosen = usize::try_from(sender.tag()).ok().and_then(|i| crate::ask::SNOOZE_CHOICES.get(i));
            if let Some((_, seconds)) = chosen {
                card.snooze(*seconds);
            }
        }

        /// Open: the decision sheet in the side panel. The question stays on
        /// the card: the sheet is answered there, then Done.
        #[unsafe(method(openDecision:))]
        fn open_decision(&self, _sender: &NSButton) {
            let Some(card) = card() else { return };
            let open = card.current.borrow().as_ref().and_then(|c| Some((c.id.clone(), c.decision_url.clone()?)));
            if let Some((id, url)) = open {
                log::info!("Ask card: open decision {}", url);
                open_in_panel(id, url);
            }
        }

        /// A button: its tag is its index in the question's buttons.
        #[unsafe(method(pressed:))]
        fn pressed(&self, sender: &NSButton) {
            let Some(card) = card() else { return };
            let index = sender.tag();
            let action = card.current.borrow().as_ref().and_then(|c| {
                usize::try_from(index).ok().and_then(|i| c.buttons.get(i).cloned())
            });
            if let Some(action) = action {
                card.answer(action);
            }
        }
    }
);

/// The body of `control:textView:doCommandBySelector:`, outside the macro:
/// an early `return` inside a `define_class!` method fights its `bool`
/// conversion.
fn enter_in_field(mtm: MainThreadMarker, text_view: &NSTextView, selector: Sel) -> bool {
    if selector != sel!(insertNewline:) {
        return false;
    }
    let Some(card) = card() else { return false };
    // Typed text sent with Enter is the free answer: it goes out as "Other" when the card has
    // that button, never under the first proposed choice.
    let typed = !card.field.stringValue().to_string().trim().is_empty();
    let Some(primary) = card.current.borrow().as_ref().map(|c| {
        match c
            .buttons
            .iter()
            .find(|b| typed && b.as_str() == crate::ask::OTHER)
        {
            Some(other) => other.clone(),
            None => c.buttons[0].clone(),
        }
    }) else {
        return false;
    };
    let flags = NSApplication::sharedApplication(mtm)
        .currentEvent()
        .map(|event| event.modifierFlags())
        .unwrap_or(NSEventModifierFlags::empty());
    if flags.contains(NSEventModifierFlags::Shift) || flags.contains(NSEventModifierFlags::Option) {
        let _: () = unsafe { msg_send![text_view, insertNewlineIgnoringFieldEditor: None::<&AnyObject>] };
        return true;
    }
    card.answer(primary);
    true
}

/// The question on screen, as much of it as the callbacks need.
struct Current {
    id: String,
    buttons: Vec<String>,
    /// What the copy control puts on the pasteboard.
    message: String,
    decision_url: Option<String>,
}

/// Everything `show` last drew, to skip a redraw when nothing changed: the tick
/// may call `show` on every frame, and redrawing would rebuild the buttons and
/// snap a card the user dragged back to the corner.
#[derive(PartialEq)]
struct Shown {
    id: String,
    from: String,
    message: String,
    buttons: Vec<String>,
    input: bool,
    asked_at: u64,
    waiting: usize,
    decision_url: Option<String>,
    /// The palette the card was drawn with: a theme change redraws it.
    light: bool,
}

struct Card {
    panel: Retained<CardPanel>,
    target: Retained<CardTarget>,
    background: Retained<NSView>,
    badge_holder: Retained<NSView>,
    who: Retained<NSTextField>,
    meta: Retained<NSTextField>,
    body: Retained<NSTextField>,
    field: Retained<NSTextField>,
    field_box: Retained<NSView>,
    /// The field's height, set from its text: it grows a line at a time as the
    /// answer gets longer.
    field_height: Retained<NSLayoutConstraint>,
    buttons: Retained<NSStackView>,
    snooze_button: Retained<NSButton>,
    line_height: CGFloat,
    current: RefCell<Option<Current>>,
    shown: RefCell<Option<Shown>>,
    /// The last question answered: it stays hidden even if `show` is called
    /// with it again before the caller has drained the answer.
    answered: RefCell<Option<String>>,
}

fn card() -> Option<Rc<Card>> {
    CARD.with(|c| c.borrow().clone())
}

fn card_or_build(mtm: MainThreadMarker) -> Rc<Card> {
    if let Some(card) = card() {
        return card;
    }
    let card = Rc::new(Card::new(mtm));
    CARD.with(|c| *c.borrow_mut() = Some(card.clone()));
    card
}

fn field_font() -> Retained<NSFont> {
    crate::window::fonts::inter(crate::window::fonts::Face::Regular, FIELD_FONT_SIZE)
}

/// A design-system colour token as an `NSColor`.
fn token(c: [f32; 3]) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c[0] as f64, c[1] as f64, c[2] as f64, 1.0)
}

fn palette() -> &'static crate::window::sidebar::tokens::Palette {
    crate::window::sidebar::tokens::get()
}

fn light() -> bool {
    std::ptr::eq(palette(), &crate::window::sidebar::tokens::LIGHT)
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// An attributed string with a font and a colour.
fn attributed(text: &str, font: &NSFont, color: &NSColor) -> Retained<NSAttributedString> {
    let keys: [&NSAttributedStringKey; 2] = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
    let attrs = NSDictionary::from_slices(&keys, &values);
    unsafe { NSAttributedString::new_with_attributes(&ns(text), &attrs) }
}

/// The attributed title of a small control, in the card's secondary style.
fn small_title(text: &str) -> Retained<NSAttributedString> {
    attributed(text, &crate::window::fonts::inter(crate::window::fonts::Face::Medium, 12.0), &token(palette().text_secondary))
}

/// A small secondary pill ("Later"), drawn by hand for the same reason as the
/// answer buttons: the card is never the key window of an app that is active,
/// so a system button would come out grey.
fn small_button(
    mtm: MainThreadMarker,
    target: &CardTarget,
    title: &str,
    action: Sel,
) -> Retained<NSButton> {
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(target), Some(action), mtm) };
    button.setBordered(false);
    button.setWantsLayer(true);
    // The design system's ghost button: no ground, the secondary ink.
    if let Some(layer) = button.layer() {
        layer.setCornerRadius(CONTROL_RADIUS);
    }
    button.setAttributedTitle(&small_title(title));
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate(&[
        // Hugs its title with 20 points of padding.
        button.widthAnchor().constraintEqualToConstant(button.attributedTitle().size().width + 20.0),
        button.heightAnchor().constraintEqualToConstant(BUTTON_H - 4.0),
    ]);
    button
}

/// Whether this key event is a plain Cmd+C (no Control, no Option: those are
/// other bindings, not a copy).
fn is_copy_key(event: &NSEvent) -> bool {
    let flags = event.modifierFlags();
    if !flags.contains(NSEventModifierFlags::Command)
        || flags.contains(NSEventModifierFlags::Control)
        || flags.contains(NSEventModifierFlags::Option)
    {
        return false;
    }
    event.charactersIgnoringModifiers().map(|c| c.to_string()) == Some("c".to_string())
}

/// What a copy puts on the pasteboard: what the user selected, and the whole
/// message when the selection is empty or blank. Pulled out of the AppKit code
/// so the choice itself is covered by a test.
fn copy_payload(selection: &str, message: &str) -> String {
    if selection.trim().is_empty() { message.to_string() } else { selection.to_string() }
}

fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}

impl Card {
    fn new(mtm: MainThreadMarker) -> Self {
        let panel: Retained<CardPanel> = {
            let this = CardPanel::alloc(mtm).set_ivars(());
            let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(WIDTH, 180.0));
            unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: rect,
                    styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            }
        };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setMovableByWindowBackground(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );

        let target: Retained<CardTarget> = {
            let this = CardTarget::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        };

        // A plain layer-backed card: the design system paints surfaces flat,
        // no vibrancy. Its colours are set in `show`, from the palette in use.
        let background = NSView::new(mtm);
        background.setWantsLayer(true);
        if let Some(layer) = background.layer() {
            layer.setCornerRadius(CARD_RADIUS);
            layer.setMasksToBounds(true);
            layer.setBorderWidth(1.0);
        }
        panel.setContentView(Some(&background));

        // Header: the asker's dot, who asks, and on the right the time and the queue.
        let badge_holder = NSView::new(mtm);
        badge_holder.setWantsLayer(true);
        if let Some(layer) = badge_holder.layer() {
            layer.setCornerRadius(DOT_D / 2.0);
        }
        let who = NSTextField::labelWithString(&ns(""), mtm);
        who.setFont(Some(&crate::window::fonts::inter(crate::window::fonts::Face::Semibold, 13.0)));
        who.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let meta = NSTextField::labelWithString(&ns(""), mtm);
        meta.setFont(Some(&crate::window::fonts::inter(crate::window::fonts::Face::Regular, 11.0)));
        meta.setContentCompressionResistancePriority_forOrientation(
            NSLayoutPriorityRequired,
            NSLayoutConstraintOrientation::Horizontal,
        );
        let spacer = NSView::new(mtm);
        let header = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[&*badge_holder, who.as_ref(), &*spacer, meta.as_ref()]),
            mtm,
        );
        header.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        header.setSpacing(8.0);
        header.setAlignment(NSLayoutAttribute::CenterY);

        let body = NSTextField::wrappingLabelWithString(&ns(""), mtm);
        body.setFont(Some(&crate::window::fonts::inter(crate::window::fonts::Face::Regular, 13.0)));
        body.setSelectable(true);
        body.setPreferredMaxLayoutWidth(WIDTH - 2.0 * INSET);

        // The answer field: borderless text inside a rounded, lightly tinted box,
        // so it reads as the place to type rather than a grey line on a grey
        // card. Wraps up to `MAX_LINES` lines; Enter sends.
        let line_height = NSLayoutManager::new().defaultLineHeightForFont(&field_font()).ceil();
        let field = NSTextField::new(mtm);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setFont(Some(&field_font()));
        field.setUsesSingleLineMode(false);
        if let Some(cell) = field.cell() {
            cell.setWraps(true);
            cell.setScrollable(false);
        }
        field.setMaximumNumberOfLines(0);
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*target))) };
        field.setTranslatesAutoresizingMaskIntoConstraints(false);
        let field_box = NSView::new(mtm);
        field_box.setWantsLayer(true);
        if let Some(layer) = field_box.layer() {
            layer.setCornerRadius(CONTROL_RADIUS);
            layer.setBorderWidth(1.0);
        }
        field_box.addSubview(&field);
        let field_height = field.heightAnchor().constraintEqualToConstant(line_height);
        activate(&[
            field.leadingAnchor().constraintEqualToAnchor_constant(&field_box.leadingAnchor(), 12.0),
            field.trailingAnchor().constraintEqualToAnchor_constant(&field_box.trailingAnchor(), -12.0),
            field.topAnchor().constraintEqualToAnchor_constant(&field_box.topAnchor(), 9.0),
            field.bottomAnchor().constraintEqualToAnchor_constant(&field_box.bottomAnchor(), -9.0),
            field_height.clone(),
        ]);

        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        buttons.setSpacing(8.0);
        let footer_spacer = NSView::new(mtm);
        // "Later" on the far left, away from the answers: putting the question
        // off is not one of them, and a mis-click must not answer the agent.
        let snooze_button = small_button(mtm, &target, "Later", sel!(showSnoozeMenu:));
        let footer = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[snooze_button.as_ref(), &*footer_spacer, buttons.as_ref()]),
            mtm,
        );
        footer.setAlignment(NSLayoutAttribute::CenterY);
        footer.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);

        let stack = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[header.as_ref(), body.as_ref(), &*field_box, footer.as_ref()]),
            mtm,
        );
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(12.0);
        stack.setCustomSpacing_afterView(10.0, &header);
        stack.setEdgeInsets(NSEdgeInsets { top: INSET - 4.0, left: INSET, bottom: INSET - 4.0, right: INSET });
        stack.setTranslatesAutoresizingMaskIntoConstraints(false);
        background.addSubview(&stack);
        activate(&[
            stack.leadingAnchor().constraintEqualToAnchor(&background.leadingAnchor()),
            stack.trailingAnchor().constraintEqualToAnchor(&background.trailingAnchor()),
            stack.topAnchor().constraintEqualToAnchor(&background.topAnchor()),
            stack.bottomAnchor().constraintEqualToAnchor(&background.bottomAnchor()),
            stack.widthAnchor().constraintEqualToConstant(WIDTH),
            // Every row spans the card's inner width: the field no longer runs past the edge.
            header.widthAnchor().constraintEqualToAnchor_constant(&stack.widthAnchor(), -2.0 * INSET),
            body.widthAnchor().constraintEqualToAnchor_constant(&stack.widthAnchor(), -2.0 * INSET),
            field_box.widthAnchor().constraintEqualToAnchor_constant(&stack.widthAnchor(), -2.0 * INSET),
            footer.widthAnchor().constraintEqualToAnchor_constant(&stack.widthAnchor(), -2.0 * INSET),
            badge_holder.widthAnchor().constraintEqualToConstant(DOT_D),
            badge_holder.heightAnchor().constraintEqualToConstant(DOT_D),
        ]);

        Card {
            panel,
            target,
            background,
            badge_holder,
            who,
            meta,
            body,
            field,
            field_box,
            field_height,
            buttons,
            snooze_button,
            line_height,
            current: RefCell::new(None),
            shown: RefCell::new(None),
            answered: RefCell::new(None),
        }
    }

    fn hide(&self) {
        *self.current.borrow_mut() = None;
        *self.shown.borrow_mut() = None;
        self.panel.orderOut(None);
    }

    /// Queue the answer and hide the card until the caller shows the next one.
    fn answer(&self, action: String) {
        let Some(id) = self.current.borrow().as_ref().map(|c| c.id.clone()) else { return };
        let text = self.field.stringValue().to_string().trim().to_string();
        log::info!("Ask card: answered {} with {:?}", id, action);
        if let Ok(mut answers) = ANSWERS.lock() {
            answers.push(CardAnswer { id: id.clone(), action, text });
        }
        *self.answered.borrow_mut() = Some(id);
        self.hide();
    }

    /// Put the message (or the selection) on the pasteboard.
    fn copy(&self) {
        let message = self.current.borrow().as_ref().map(|c| c.message.clone()).unwrap_or_default();
        let text = copy_payload(&self.selected_text(), &message);
        if text.is_empty() {
            return;
        }
        crate::appkit_input::copy_to_pasteboard(&text);
        log::info!("Ask card: copied {} characters", text.chars().count());
    }

    /// The text selected in the card, in the message or in the answer field.
    /// Taken from the field editor through `NSString`, never by slicing a Rust
    /// `String`: an `NSRange` counts UTF-16 units, not bytes.
    fn selected_text(&self) -> String {
        let Some(responder) = self.panel.firstResponder() else { return String::new() };
        let Ok(view) = responder.downcast::<NSTextView>() else { return String::new() };
        let range = view.selectedRange();
        if range.length == 0 {
            return String::new();
        }
        let string = view.string();
        if range.location + range.length > string.length() {
            return String::new();
        }
        string.substringWithRange(range).to_string()
    }

    /// Queue the snooze and hide the card. The board puts the question back on
    /// it once the delay is over; the agent is not told and keeps waiting, so
    /// nothing is lost if the user never comes back to it.
    fn snooze(&self, seconds: u64) {
        let Some(id) = self.current.borrow().as_ref().map(|c| c.id.clone()) else { return };
        log::info!("Ask card: {} put off for {} s", id, seconds);
        if let Ok(mut snoozes) = SNOOZES.lock() {
            snoozes.push(CardSnooze { id, seconds });
        }
        // Deliberately not marked as answered: the question comes back later,
        // and a card that still thought it was answered would stay blank.
        self.hide();
    }

    fn show(&self, mtm: MainThreadMarker, question: &CardQuestion, waiting: usize) {
        if self.answered.borrow().as_deref() == Some(question.id) {
            return;
        }
        let shown = Shown {
            id: question.id.to_string(),
            from: question.from.to_string(),
            message: question.message.to_string(),
            buttons: question.buttons.to_vec(),
            input: question.input,
            asked_at: question.asked_at,
            waiting,
            decision_url: question.decision_url.map(str::to_string),
            light: light(),
        };
        if self.shown.borrow().as_ref() == Some(&shown) && self.panel.isVisible() {
            return;
        }
        let is_new = self.current.borrow().as_ref().map(|c| c.id.as_str()) != Some(question.id);
        *self.current.borrow_mut() = Some(Current {
            id: shown.id.clone(),
            buttons: shown.buttons.clone(),
            message: shown.message.clone(),
            decision_url: shown.decision_url.clone(),
        });
        *self.shown.borrow_mut() = Some(shown);

        let p = palette();
        // The panel's appearance follows the palette, so the field's caret and
        // selection read on the card's ground.
        let appearance_name = unsafe {
            if light() { objc2_app_kit::NSAppearanceNameAqua } else { objc2_app_kit::NSAppearanceNameDarkAqua }
        };
        objc2_app_kit::NSAppearanceCustomization::setAppearance::<>(self.panel.as_ref() as &NSWindow, objc2_app_kit::NSAppearance::appearanceNamed(appearance_name).as_deref());
        if let Some(layer) = self.background.layer() {
            layer.setBackgroundColor(Some(&token(p.tile).CGColor()));
            layer.setBorderColor(Some(&token(p.border_subtle).CGColor()));
        }
        let decision = question.decision_url.is_some();
        // A decision names what it is; the asker moves to the meta line.
        self.who.setStringValue(&ns(if decision { "New decision" } else { question.from }));
        self.who.setTextColor(Some(&token(p.text_primary)));
        let (r, g, b) = crate::ask::agent_rgb(question.from);
        let tint = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0);
        if let Some(layer) = self.badge_holder.layer() {
            let dot = if decision { token(p.awaiting) } else { tint };
            layer.setBackgroundColor(Some(&dot.CGColor()));
        }
        let time = short_time(question.asked_at);
        let mut meta = if decision { format!("{} · {}", question.from, time) } else { time };
        if waiting > 0 {
            meta = format!("{} · {} more", meta, waiting);
        }
        self.meta.setStringValue(&ns(&meta));
        self.meta.setTextColor(Some(&token(p.text_tertiary)));
        self.body.setStringValue(&ns(question.message));
        self.body.setTextColor(Some(&token(p.text_primary)));
        if is_new {
            self.field.setStringValue(&ns(""));
            self.field_height.setConstant(self.line_height);
        }
        self.field_box.setHidden(!question.input);
        self.field.setTextColor(Some(&token(p.text_primary)));
        if let Some(layer) = self.field_box.layer() {
            layer.setBackgroundColor(Some(&token(p.ground).CGColor()));
            layer.setBorderColor(Some(&token(p.border_strong).CGColor()));
        }
        self.field.setPlaceholderAttributedString(Some(&attributed(PLACEHOLDER, &field_font(), &token(p.text_tertiary))));
        self.snooze_button.setAttributedTitle(&small_title("Later"));

        for view in self.buttons.arrangedSubviews().iter() {
            view.removeFromSuperview();
        }
        // Right-aligned, the default (first action) last, as macOS dialogs do.
        // A decision's Open comes first: it does not answer.
        if decision {
            let open = self.make_button(mtm, "Open", None);
            self.buttons.addArrangedSubview(&open);
        }
        for (index, title) in question.buttons.iter().enumerate().rev() {
            let button = self.make_button(mtm, title, Some(index));
            self.buttons.addArrangedSubview(&button);
        }

        self.background.layoutSubtreeIfNeeded();
        let fitting = self.background.fittingSize();
        let size = CGSize::new(WIDTH, fitting.height);
        let screen = NSScreen::mainScreen(mtm)
            .map(|s| s.visibleFrame())
            .unwrap_or(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1440.0, 900.0)));
        let frame = CGRect::new(
            CGPoint::new(
                screen.origin.x + screen.size.width - size.width - 16.0,
                screen.origin.y + screen.size.height - size.height - 16.0,
            ),
            size,
        );
        self.panel.setFrame_display(frame, true);
        self.panel.orderFrontRegardless();
        if is_new {
            if let Some(sound) = NSSound::soundNamed(&ns("Glass")) {
                sound.play();
            }
            NSApplication::sharedApplication(mtm)
                .requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
        }
    }

    /// Drawn by hand: macOS paints its default button blue only in the key
    /// window, and the card never takes the keyboard on its own, so system
    /// buttons all came out grey. The design system's two variants: the first
    /// answer is primary (accent fill), the others secondary (`btn2` fill, a
    /// `b2` rim). `index` is the answer's index; `None` is a decision's Open.
    fn make_button(&self, mtm: MainThreadMarker, title: &str, index: Option<usize>) -> Retained<NSButton> {
        let primary = index == Some(0);
        let p = palette();
        let action = if index.is_some() { sel!(pressed:) } else { sel!(openDecision:) };
        let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(&self.target), Some(action), mtm) };
        button.setTag(index.map_or(-1, |i| i as isize));
        button.setBordered(false);
        button.setWantsLayer(true);
        if let Some(layer) = button.layer() {
            layer.setCornerRadius(CONTROL_RADIUS);
            if primary {
                layer.setBackgroundColor(Some(&token(p.accent).CGColor()));
            } else {
                layer.setBackgroundColor(Some(&token(p.tile_hover).CGColor()));
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(&token(p.border_strong).CGColor()));
            }
        }
        let face = if primary { crate::window::fonts::Face::Semibold } else { crate::window::fonts::Face::Medium };
        let color = if primary { token(p.text_on_fill) } else { token(p.text_primary) };
        button.setAttributedTitle(&attributed(title, &crate::window::fonts::inter(face, 13.0), &color));
        if primary {
            button.setKeyEquivalent(&ns("\r"));
        }
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        let width = button.attributedTitle().size().width + 28.0;
        activate(&[
            button.widthAnchor().constraintEqualToConstant(width.max(72.0)),
            button.heightAnchor().constraintEqualToConstant(BUTTON_H),
        ]);
        button
    }

    /// Grows (or shrinks) the field to its text, up to `MAX_LINES`, and the card
    /// with it, its top edge fixed so the card extends downward like a chat input.
    fn fit_field(&self) {
        let mut text = match self.field.currentEditor() {
            Some(editor) => editor.string().to_string(),
            None => self.field.stringValue().to_string(),
        };
        // A trailing newline starts a line the measure would not count.
        if text.ends_with('\n') || text.is_empty() {
            text.push(' ');
        }
        let width = (self.field.bounds().size.width - 4.0).max(1.0);
        let measured = attributed(&text, &field_font(), &NSColor::labelColor())
            .boundingRectWithSize_options_context(
                CGSize::new(width, CGFloat::MAX),
                NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::UsesFontLeading,
                None,
            )
            .size
            .height;
        let height = measured.ceil().max(self.line_height).min(self.line_height * MAX_LINES);
        if (height - self.field_height.constant()).abs() < 1.0 {
            return;
        }
        self.field_height.setConstant(height);
        self.background.layoutSubtreeIfNeeded();
        let new_height = self.background.fittingSize().height;
        let mut frame = self.panel.frame();
        frame.origin.y = frame.origin.y + frame.size.height - new_height;
        frame.size.height = new_height;
        self.panel.setFrame_display(frame, true);
    }
}

/// `asked_at` as the local short time (`14:05`, or `2:05 PM` in a 12-hour
/// locale), as Swift's `DateFormatter.localizedString(timeStyle: .short)`.
fn short_time(asked_at: u64) -> String {
    let date = NSDate::dateWithTimeIntervalSince1970(asked_at as f64);
    NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        NSDateFormatterStyle::NoStyle,
        NSDateFormatterStyle::ShortStyle,
    )
    .to_string()
}

/// Show `question` on the card (replacing what it showed), or hide the card
/// when `None`. `waiting` is how many more questions queue behind it.
///
/// Also sets the Dock badge to the total count (`waiting + 1`), or clears it.
/// Calling it again with the same id keeps what the user is typing.
pub fn show(mtm: MainThreadMarker, question: Option<&CardQuestion>, waiting: usize) {
    let count = question.map(|_| waiting + 1);
    if BADGE.with(|b| b.replace(count)) != count {
        let label = count.map(|n| ns(&n.to_string()));
        NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref());
    }
    match question {
        Some(question) => card_or_build(mtm).show(mtm, question, waiting),
        None => {
            if let Some(card) = card() {
                card.hide();
                *card.answered.borrow_mut() = None;
            }
        }
    }
}

/// Answers given since the last call (button clicked or Enter).
pub fn take_answers() -> Vec<CardAnswer> {
    match ANSWERS.lock() {
        Ok(mut answers) => std::mem::take(&mut *answers),
        Err(_) => Vec::new(),
    }
}

/// Queue question `id`'s sheet (`link`) for the front window's side panel.
/// Opened, it leaves the floating card: it is answered on the sheet, and
/// stays in the sidebar's Asks.
pub fn open_in_panel(id: String, link: String) {
    if let Ok(mut opens) = PANEL_OPENS.lock() {
        opens.push((id, link));
    }
}

/// Decision sheets the user clicked Open on since the last call: the tick
/// shows each in the front window's side panel.
pub fn take_panel_opens() -> Vec<(String, String)> {
    match PANEL_OPENS.lock() {
        Ok(mut opens) => std::mem::take(&mut *opens),
        Err(_) => Vec::new(),
    }
}

/// Snoozes chosen since the last call.
pub fn take_snoozes() -> Vec<CardSnooze> {
    match SNOOZES.lock() {
        Ok(mut snoozes) => std::mem::take(&mut *snoozes),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::copy_payload;

    #[test]
    fn copying_takes_the_selection_when_there_is_one() {
        assert_eq!(copy_payload("/tmp/x", "run /tmp/x then tell me"), "/tmp/x");
        assert_eq!(copy_payload("", "the whole message"), "the whole message");
        assert_eq!(
            copy_payload("  \n ", "the whole message"),
            "the whole message",
            "a drag that selected only whitespace is not a selection"
        );
    }
}
