//! The side panel: a web view down the right edge of the window, the mirror
//! of the sidebar on the left. It shows what a click in Kova points at that
//! is not a terminal: a decision sheet (an artifact on claude.ai), a website,
//! a markdown file rendered in the design system's colours.
//!
//! The window's content view holds three siblings: the sidebar, the Metal
//! view and this panel. `KovaView::apply_layout` splits the width between
//! them (`split`, pure, tested); the terminal never reserves space for the
//! panel, its viewport is its own frame.
//!
//! The web view uses WebKit's default data store, which is persistent: a
//! login made in the panel (claude.ai, Google) is kept across launches, so
//! the user logs in once. Kova never types a password or approves a consent
//! screen for them.
//!
//! What may be opened (`target_for`): http(s) URLs (`ask::is_panel_url`), and
//! existing local `.md` / `.html` files. A markdown file is rendered here
//! with raw HTML escaped, so a file cannot run a script in the panel; it is
//! rendered again when it changes on disk. See `docs/side-panel.md`.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSCursor, NSEvent, NSFont, NSLineBreakMode, NSMutableParagraphStyle,
    NSStringDrawing, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWorkspace,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSObject, NSObjectProtocol, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType, WKScriptMessage,
    WKScriptMessageHandler, WKUIDelegate, WKUserContentController,
    WKWebView, WKWebpagePreferences,
    WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
};

use super::feather::Icon;
use super::sidebar::tokens;
use super::sidebar_view::{fill_round, stroke_icon, Rect};

// ---------------------------------------------------------------
// Metrics (points)
// ---------------------------------------------------------------

/// The header: as tall as the sidebar's top row, so the two read as one band.
pub const HEADER_H: f64 = 38.0;
/// The resize zone along the left edge, separator included.
pub const EDGE_W: f64 = 5.0;
const SEP_W: f64 = 1.0;
const BTN_W: f64 = 26.0;
const BTN_H: f64 = 26.0;
const BTN_GAP: f64 = 2.0;
const BTN_ICON: f64 = 14.0;
const BTN_RADIUS: f64 = 6.0;
const PAD_RIGHT: f64 = 10.0;
const TITLE_X: f64 = 14.0;
const TITLE_SIZE: f64 = 13.0;
/// The narrowest panel worth opening.
pub const PANEL_MIN_W: f64 = 320.0;
/// The share of the room right of the sidebar the panel takes, bounds.
pub const FRACTION_RANGE: (f32, f32) = (0.2, 0.8);
pub const DEFAULT_FRACTION: f32 = 0.5;
/// Safari's own tokens after WebKit's: a site that sniffs the user agent
/// treats the panel as Safari rather than as an unknown embedded browser.
const USER_AGENT_APP: &str = "Version/18.5 Safari/605.1.15";

// ---------------------------------------------------------------
// The panel's width, one setting for the whole app (like the sidebar's)
// ---------------------------------------------------------------

static FRACTION_BITS: AtomicU32 = AtomicU32::new(0x3f00_0000); // 0.5

pub fn clamp_fraction(f: f32) -> f32 {
    if !f.is_finite() {
        return DEFAULT_FRACTION;
    }
    f.clamp(FRACTION_RANGE.0, FRACTION_RANGE.1)
}

/// Adopt the fraction `prefs.json` remembered. Called once at startup.
pub fn init(fraction: Option<f32>) {
    set_fraction(fraction.unwrap_or(DEFAULT_FRACTION));
}

pub fn fraction() -> f32 {
    f32::from_bits(FRACTION_BITS.load(Ordering::Relaxed))
}

/// Set the fraction (clamped). Not persisted here: an edge drag calls this
/// on every event and `sidebar::persist` once, on mouse up.
pub fn set_fraction(f: f32) {
    FRACTION_BITS.store(clamp_fraction(f).to_bits(), Ordering::Relaxed);
}

// ---------------------------------------------------------------
// What the panel shows
// ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelTarget {
    /// An http(s) page: a site, an artifact.
    Web(String),
    /// A markdown file, rendered by Kova.
    Markdown(PathBuf),
    /// A local HTML file, loaded as is.
    Html(PathBuf),
    /// Any other local file WebKit shows itself (an image, a PDF, text,
    /// JSON), loaded as is with no script.
    File(PathBuf),
    /// A decision sheet Kova renders itself (`kova ask --decisions`), by
    /// the id of its question.
    Sheet(String),
    /// A long `kova ask` question, read and answered in full, by its id.
    Question(String),
    /// What the panel opened lately (`panel_history`).
    History,
}

impl PanelTarget {
    /// The title the header shows before the page names itself.
    pub fn fallback_title(&self) -> String {
        match self {
            PanelTarget::Web(url) => url.clone(),
            PanelTarget::Markdown(p) | PanelTarget::Html(p) | PanelTarget::File(p) => {
                p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            }
            PanelTarget::Sheet(_) => "Decision".to_string(),
            PanelTarget::Question(_) => "Question".to_string(),
            PanelTarget::History => "History".to_string(),
        }
    }

    /// What a card's Open link names: `kova-sheet:<id>` for a native sheet,
    /// else an http(s) page.
    pub fn from_link(link: &str) -> PanelTarget {
        if let Some(id) = link.strip_prefix(crate::ask::QUESTION_LINK) {
            return PanelTarget::Question(id.to_string());
        }
        match link.strip_prefix(crate::decision_sheet::LINK_SCHEME) {
            Some(id) => PanelTarget::Sheet(id.to_string()),
            None => PanelTarget::Web(link.to_string()),
        }
    }

    /// What "Open in the browser" opens: the URL, or the file itself.
    pub fn external(&self) -> String {
        match self {
            PanelTarget::Web(url) => url.clone(),
            PanelTarget::Markdown(p) | PanelTarget::Html(p) | PanelTarget::File(p) => p.to_string_lossy().into_owned(),
            PanelTarget::Sheet(_) | PanelTarget::Question(_) | PanelTarget::History => String::new(),
        }
    }
}

/// The extensions `file_kind` knows, lower case: what the chat looks for to
/// link a file's path (`chat::links`).
pub const FILE_EXTENSIONS: &[&str] = &["md", "markdown", "html", "htm", "png", "jpg", "jpeg", "gif", "webp", "svg", "pdf", "txt", "log", "json"];

/// Whether a local file is one the panel shows, by its extension.
fn file_kind(path: &Path) -> Option<fn(PathBuf) -> PanelTarget> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => Some(PanelTarget::Markdown),
        "html" | "htm" => Some(PanelTarget::Html),
        // A local file a card or an agent points at opens here, never as a
        // dead path (Robin, 10/11/2026).
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "pdf" | "txt" | "log" | "json" => Some(PanelTarget::File),
        _ => None,
    }
}

/// The local file a link names, resolved: `file://` stripped, `~` expanded,
/// a relative path joined to `cwd`. Pure: the caller checks it exists.
pub fn resolve_path(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let raw = raw.trim();
    let raw = raw.strip_prefix("file://").unwrap_or(raw);
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    // A `path:12` or `path:12:3` location suffix, as compilers and agents print it.
    let raw = strip_location(raw);
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        home?.join(rest)
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        cwd?.join(raw)
    };
    Some(path)
}

fn strip_location(raw: &str) -> &str {
    let mut s = raw;
    for _ in 0..2 {
        if let Some((head, tail)) = s.rsplit_once(':')
            && !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_digit())
        {
            s = head;
        }
    }
    s
}

/// What the panel would show for a link or a path, if anything: an http(s)
/// URL, or an existing `.md` / `.html` file. `exists` is the filesystem test,
/// a parameter so the rule is testable.
pub fn target_for(raw: &str, cwd: Option<&Path>, home: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PanelTarget> {
    let raw = raw.trim();
    if crate::ask::is_panel_url(raw) {
        return Some(PanelTarget::Web(raw.to_string()));
    }
    let path = resolve_path(raw, cwd, home)?;
    let make = file_kind(&path)?;
    if exists(&path) {
        return Some(make(path));
    }
    // A relative path printed from a parent folder (`Claap/Product/docs/x.md`
    // in a pane at `~/AI directory/Claap`): the pane's ancestors, up to and
    // including `$HOME`, the nearest existing file first.
    let rel = strip_location(raw.strip_prefix("file://").unwrap_or(raw));
    if rel.starts_with('/') || rel.starts_with('~') {
        return None;
    }
    let (cwd, home) = (cwd?, home?);
    if !cwd.starts_with(home) {
        return None;
    }
    cwd.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(home))
        .map(|dir| dir.join(rel))
        .find(|p| exists(p))
        .map(make)
}

/// `target_for` against the real filesystem and `$HOME`.
pub fn target_on_disk(raw: &str, cwd: Option<&Path>) -> Option<PanelTarget> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    target_for(raw, cwd, home.as_deref(), |p| p.is_file())
}

/// What `Ouvrir` would show for an attention entry's action: `target_for`
/// (an http(s) link, or an existing `.md` / `.html` file, `~/` read as home),
/// after the `#anchor` of a file is stripped. No working folder: a relative
/// path names nothing here. Plain words show nothing.
pub fn open_target(action: &str, home: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> Option<PanelTarget> {
    let action = action.trim();
    if crate::ask::is_panel_url(action) {
        return Some(PanelTarget::Web(action.to_string()));
    }
    let file = match action.rfind('#') {
        Some(at) if !action[at..].contains('/') => &action[..at],
        _ => action,
    };
    target_for(file, None, home, exists)
}

// ---------------------------------------------------------------
// The window's split, pure
// ---------------------------------------------------------------

/// How the container's width is shared: the sidebar at the left (or not),
/// the terminal in the middle, the panel at the right (or not). x and width
/// of each, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub sidebar: bool,
    pub sidebar_w: f64,
    pub term_x: f64,
    pub term_w: f64,
    /// x and width of the panel, when it is shown.
    pub panel: Option<(f64, f64)>,
}

/// Share `container_w` between the sidebar (`sidebar_w`, when wanted), the
/// terminal (never under `min_split`) and the panel (`fraction` of what the
/// sidebar leaves, at least `PANEL_MIN_W`, when wanted). When the three do not
/// fit, the sidebar goes first, then the panel: the terminal always keeps
/// `min_split`, as it does with the sidebar alone.
pub fn split(container_w: f64, sidebar_w: f64, sidebar_wanted: bool, panel_wanted: bool, fraction: f32, min_split: f64) -> Split {
    let panel_w_for = |room: f64| -> Option<f64> {
        if !panel_wanted {
            return None;
        }
        let wanted = (room * clamp_fraction(fraction) as f64).round().max(PANEL_MIN_W);
        let w = wanted.min(room - min_split);
        (w >= PANEL_MIN_W).then_some(w)
    };
    let with_sidebar = sidebar_wanted && container_w - sidebar_w >= min_split;
    let (sidebar, left, panel_w) = if with_sidebar {
        let room = container_w - sidebar_w;
        match panel_w_for(room) {
            Some(w) => (true, sidebar_w, Some(w)),
            // The panel does not fit beside the sidebar: the sidebar yields.
            None if panel_wanted && panel_w_for(container_w).is_some() => (false, 0.0, panel_w_for(container_w)),
            None => (true, sidebar_w, None),
        }
    } else {
        (false, 0.0, panel_w_for(container_w))
    };
    let panel = panel_w.map(|w| (container_w - w, w));
    let term_w = (container_w - left - panel_w.unwrap_or(0.0)).max(1.0);
    Split { sidebar, sidebar_w, term_x: left, term_w, panel }
}

/// The fraction an edge drag at `panel_w` stands for, in the room right of
/// the sidebar.
pub fn fraction_for(panel_w: f64, room: f64) -> f32 {
    if room <= 0.0 {
        return DEFAULT_FRACTION;
    }
    clamp_fraction((panel_w / room) as f32)
}

// ---------------------------------------------------------------
// The header, pure
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelHit {
    #[default]
    None,
    History,
    Back,
    External,
    Close,
    /// The resize zone along the left edge.
    Edge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderLayout {
    pub title: Rect,
    pub history: Rect,
    pub back: Rect,
    pub external: Rect,
    pub close: Rect,
}

impl HeaderLayout {
    /// The header of a panel `w` wide: the four ghost buttons at the right
    /// (history, back, open in the browser, close), the title on the rest.
    pub fn new(w: f64) -> Self {
        let y = (HEADER_H - BTN_H) / 2.0;
        let close = Rect::new(w - PAD_RIGHT - BTN_W, y, BTN_W, BTN_H);
        let external = Rect::new(close.x - BTN_GAP - BTN_W, y, BTN_W, BTN_H);
        let back = Rect::new(external.x - BTN_GAP - BTN_W, y, BTN_W, BTN_H);
        let history = Rect::new(back.x - BTN_GAP - BTN_W, y, BTN_W, BTN_H);
        let title = Rect::new(TITLE_X, 0.0, (history.x - 8.0 - TITLE_X).max(0.0), HEADER_H);
        HeaderLayout { title, history, back, external, close }
    }

    /// What a point of the panel (flipped, panel coordinates) is on.
    pub fn hit(&self, x: f64, y: f64) -> PanelHit {
        if x < EDGE_W {
            return PanelHit::Edge;
        }
        if y >= HEADER_H {
            return PanelHit::None;
        }
        if self.close.contains(x, y) {
            PanelHit::Close
        } else if self.external.contains(x, y) {
            PanelHit::External
        } else if self.back.contains(x, y) {
            PanelHit::Back
        } else if self.history.contains(x, y) {
            PanelHit::History
        } else {
            PanelHit::None
        }
    }
}

/// The header's text: the page's own title, then always the host it comes
/// from for a web page, so a page cannot pass itself off as another site by
/// its title. `None` until the page has a title or an address.
pub fn header_title(title: Option<&str>, url: Option<&str>) -> Option<String> {
    let host = url.filter(|u| crate::ask::is_panel_url(u)).and_then(|u| {
        let rest = u.split_once("://")?.1;
        let host = rest.split(['/', '?', '#']).next()?;
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        (!host.is_empty()).then(|| host.to_string())
    });
    match (title.map(str::trim).filter(|t| !t.is_empty()), host) {
        (Some(t), Some(h)) => Some(format!("{} · {}", t, h)),
        (None, Some(h)) => Some(h),
        (Some(t), None) => Some(t.to_string()),
        (None, None) => None,
    }
}

// ---------------------------------------------------------------
// Markdown, pure
// ---------------------------------------------------------------

fn css_hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// A target with no scheme: a relative path, an anchor, a query.
fn relative_target(dest: &str) -> bool {
    let d = dest.trim();
    let before = d.split(['/', '?', '#']).next().unwrap_or("");
    !before.contains(':') && !d.starts_with("//")
}

/// What a link in a rendered markdown file may point at: http(s), mailto, or
/// a relative target. Never `javascript:` or `data:`, which would run in the page.
fn safe_link(dest: &str) -> bool {
    let lower = dest.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || relative_target(dest)
}

/// A markdown document as a page in the design system's colours (`palette`):
/// Inter, the text and surface tokens, code on `bg2`. Raw HTML in the source
/// is shown as text, never interpreted: a file cannot run a script here.
pub fn render_markdown(source: &str, title: &str, palette: &tokens::Palette) -> String {
    use pulldown_cmark::{html, Event, Options, Parser, Tag};
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let parser = Parser::new_ext(source, options).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        // A link keeps only a safe target; an image only a local one (a remote
        // image would tell its server the file was opened).
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let dest_url = if safe_link(&dest_url) { dest_url } else { "".into() };
            Event::Start(Tag::Link { link_type, dest_url, title, id })
        }
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            let dest_url = if relative_target(&dest_url) { dest_url } else { "".into() };
            Event::Start(Tag::Image { link_type, dest_url, title, id })
        }
        other => other,
    });
    let mut body = String::new();
    html::push_html(&mut body, parser);
    let p = palette;
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>{title}</title>
<style>
:root {{ color-scheme: {scheme}; }}
html {{ background: {bg}; }}
body {{ margin: 0; padding: 24px 28px 48px; font: 14px/1.6 "Inter", -apple-system, system-ui, sans-serif;
  color: {t1}; background: {bg}; max-width: 860px; }}
h1, h2, h3, h4 {{ color: {t1}; font-weight: 600; line-height: 1.3; margin: 1.6em 0 .6em; }}
h1 {{ font-size: 22px; margin-top: 0; }} h2 {{ font-size: 18px; }} h3 {{ font-size: 15px; }} h4 {{ font-size: 14px; }}
p, li {{ color: {t2}; }}
a {{ color: {accent}; text-decoration: none; }} a:hover {{ text-decoration: underline; }}
strong {{ color: {t1}; }}
code {{ font: 12.5px/1.5 "SF Mono", Menlo, monospace; background: {bg2}; border: 1px solid {b3};
  border-radius: 4px; padding: 1px 4px; }}
pre {{ background: {bg2}; border: 1px solid {b3}; border-radius: 8px; padding: 12px 14px; overflow-x: auto; }}
pre code {{ background: none; border: 0; padding: 0; }}
blockquote {{ margin: 1em 0; padding: 2px 14px; border-left: 3px solid {b2}; color: {t3}; }}
table {{ border-collapse: collapse; margin: 1em 0; font-size: 13px; }}
th, td {{ border: 1px solid {b3}; padding: 6px 10px; text-align: left; vertical-align: top; }}
th {{ background: {bg2}; color: {t1}; font-weight: 600; }}
hr {{ border: 0; border-top: 1px solid {b3}; margin: 2em 0; }}
img {{ max-width: 100%; border-radius: 8px; }}
input[type=checkbox] {{ margin-right: 6px; }}
</style></head><body>
{body}</body></html>"#,
        title = escape_html(title),
        scheme = if p == &tokens::LIGHT { "light" } else { "dark" },
        bg = css_hex(p.ground),
        bg2 = css_hex(p.bg2),
        t1 = css_hex(p.text_primary),
        t2 = css_hex(p.text_secondary),
        t3 = css_hex(p.text_tertiary),
        b2 = css_hex(p.border_strong),
        b3 = css_hex(p.border_subtle),
        accent = css_hex(p.sel_rail),
        body = body,
    )
}

// ---------------------------------------------------------------
// AppKit
// ---------------------------------------------------------------

pub struct SidePanelIvars {
    web: OnceCell<Retained<WKWebView>>,
    delegate: OnceCell<Retained<PanelWebDelegate>>,
    /// What the panel shows; `None` until something is opened.
    current: RefCell<Option<PanelTarget>>,
    /// The sheet or long question a link was opened from: Back on the first
    /// page shows it again (it is not in the web view's own history).
    return_to: RefCell<Option<PanelTarget>>,
    /// Whether the user wants the panel on screen (toggle, close button).
    open: Cell<bool>,
    title: RefCell<String>,
    hovered: Cell<PanelHit>,
    pressed: Cell<PanelHit>,
    /// An edge drag: the panel's right edge in window coordinates and the room
    /// right of the sidebar, both fixed for the drag.
    drag: Cell<Option<(f64, f64)>>,
    /// A rendered markdown file's modification time, to render it again when
    /// it changes.
    rendered_mtime: Cell<Option<std::time::SystemTime>>,
    last_mtime_check: Cell<Option<std::time::Instant>>,
    /// The theme the page Kova rendered was drawn in (`theme::generation`).
    theme: Cell<u64>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaSidePanelView"]
    #[ivars = SidePanelIvars]
    pub struct SidePanelView;

    unsafe impl NSObjectProtocol for SidePanelView {}

    impl SidePanelView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: CGSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.layout_web();
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: CGRect) {
            self.draw();
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let b = self.bounds();
            let edge = Rect::new(0.0, 0.0, EDGE_W, b.size.height);
            #[allow(deprecated)]
            self.addCursorRect_cursor(cg(&edge), &NSCursor::resizeLeftRightCursor());
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let old: Vec<_> = self.trackingAreas().to_vec();
            for area in &old {
                self.removeTrackingArea(area);
            }
            let options = NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveInKeyWindow
                | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe {
                let alloc: objc2::rc::Allocated<NSTrackingArea> = msg_send![objc2::class!(NSTrackingArea), alloc];
                NSTrackingArea::initWithRect_options_owner_userInfo(alloc, self.bounds(), options, Some(self.as_ref()), None)
            };
            self.addTrackingArea(&area);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = self.local(event);
            let hit = self.header().hit(x, y);
            let hit = if hit == PanelHit::Edge { PanelHit::None } else { hit };
            if self.ivars().hovered.replace(hit) != hit {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            if self.ivars().hovered.replace(PanelHit::None) != PanelHit::None {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let (x, y) = self.local(event);
            let hit = self.header().hit(x, y);
            if hit == PanelHit::Edge {
                let frame = self.frame();
                let right = frame.origin.x + frame.size.width;
                let room = super::sidebar_ui::room_right_of_sidebar(self);
                self.ivars().drag.set(Some((right, room)));
                return;
            }
            self.ivars().pressed.set(hit);
            if hit == PanelHit::None && y < HEADER_H && event.clickCount() == 2 {
                // The header is a title bar: a double click zooms the window.
                if let Some(win) = self.window() {
                    win.performZoom(None);
                }
            } else if hit == PanelHit::None && y < HEADER_H {
                if let Some(win) = self.window() {
                    win.performWindowDragWithEvent(event);
                }
            }
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some((right, room)) = self.ivars().drag.get() else { return };
            let Some(superview) = (unsafe { self.superview() }) else { return };
            let p = superview.convertPoint_fromView(event.locationInWindow(), None);
            let w = (right - p.x).max(PANEL_MIN_W);
            set_fraction(fraction_for(w, room));
            if let Some(win) = self.window()
                && let Some(kova) = crate::app::kova_view(&win)
            {
                kova.apply_layout();
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if self.ivars().drag.take().is_some() {
                super::sidebar::persist();
                return;
            }
            let pressed = self.ivars().pressed.replace(PanelHit::None);
            let (x, y) = self.local(event);
            if pressed != PanelHit::None && self.header().hit(x, y) == pressed {
                match pressed {
                    PanelHit::History => self.show_history(),
                    PanelHit::Back => self.go_back(),
                    PanelHit::External => self.open_externally(),
                    PanelHit::Close => self.close(),
                    PanelHit::None | PanelHit::Edge => {}
                }
            }
            self.setNeedsDisplay(true);
        }
    }
);

impl SidePanelView {
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SidePanelIvars {
            web: OnceCell::new(),
            delegate: OnceCell::new(),
            current: RefCell::new(None),
            return_to: RefCell::new(None),
            open: Cell::new(false),
            title: RefCell::new(String::new()),
            hovered: Cell::new(PanelHit::None),
            pressed: Cell::new(PanelHit::None),
            drag: Cell::new(None),
            rendered_mtime: Cell::new(None),
            last_mtime_check: Cell::new(None),
            theme: Cell::new(crate::theme::generation()),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setHidden(true);
        view
    }

    /// Whether the panel wants to be on screen: something to show, not closed.
    pub fn wanted(&self) -> bool {
        self.ivars().open.get() && self.ivars().current.borrow().is_some()
    }

    /// Show `target` in the panel, opening it.
    pub fn open_target(&self, target: PanelTarget) {
        log::info!("side panel: open {:?}", target);
        self.ivars().return_to.borrow_mut().take();
        let mtm = MainThreadMarker::from(self);
        let web = self.web_view(mtm);
        *self.ivars().title.borrow_mut() = target.fallback_title();
        self.ivars().rendered_mtime.set(None);
        match &target {
            PanelTarget::Web(url) => {
                if let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) {
                    let request = NSURLRequest::requestWithURL(&nsurl);
                    unsafe { web.loadRequest(&request) };
                }
            }
            PanelTarget::Markdown(path) => self.render_markdown_file(&web, path),
            PanelTarget::Html(path) | PanelTarget::File(path) => {
                let file = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                let dir = path.parent().unwrap_or(Path::new("/"));
                let read = NSURL::fileURLWithPath(&NSString::from_str(&dir.to_string_lossy()));
                unsafe { web.loadFileURL_allowingReadAccessToURL(&file, &read) };
            }
            PanelTarget::Sheet(id) => self.render_sheet(mtm, &web, id),
            PanelTarget::Question(id) => self.render_question(mtm, &web, id),
            PanelTarget::History => {
                let html = super::panel_history::page();
                unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
            }
        }
        let title = self.ivars().title.borrow().clone();
        let fallback = target.fallback_title();
        super::panel_history::record(&target, if title == fallback { "" } else { &title });
        *self.ivars().current.borrow_mut() = Some(target);
        self.ivars().open.set(true);
        self.relayout();
        self.setNeedsDisplay(true);
    }

    /// The sheet `id` was answered (or is gone): if the panel shows it, it
    /// shows what was sent instead of the form.
    pub fn sheet_closed(&self, id: &str, title: &str, summary: Option<&str>) {
        if !matches!(self.ivars().current.borrow().as_ref(), Some(PanelTarget::Sheet(s) | PanelTarget::Question(s)) if s == id) {
            return;
        }
        let Some(web) = self.ivars().web.get() else { return };
        let html = crate::decision_sheet::render_closed(title, summary, tokens::get());
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
    }

    /// The panel's history list, opening the panel on it (the header's clock
    /// button, View > Panel History).
    pub fn show_history(&self) {
        self.open_target(PanelTarget::History);
    }

    /// The id of the sheet the panel shows, if it shows one.
    fn sheet_id(&self) -> Option<String> {
        match self.ivars().current.borrow().as_ref() {
            Some(PanelTarget::Sheet(id)) => Some(id.clone()),
            _ => None,
        }
    }

    /// The id of the long question the panel shows, if it shows one.
    fn question_id(&self) -> Option<String> {
        match self.ivars().current.borrow().as_ref() {
            Some(PanelTarget::Question(id)) => Some(id.clone()),
            _ => None,
        }
    }

    /// Hide the panel, keeping its page: the toggle brings it back as it was.
    pub fn close(&self) {
        self.ivars().open.set(false);
        self.relayout();
    }

    /// The View menu's toggle: close an open panel, reopen a closed one that
    /// has a page. Returns false when there is nothing to show.
    pub fn toggle(&self) -> bool {
        if self.wanted() {
            self.close();
            return true;
        }
        if self.ivars().current.borrow().is_none() {
            return false;
        }
        self.ivars().open.set(true);
        self.relayout();
        true
    }

    /// Once per tick while shown: follow the page's title, and render a
    /// markdown file again when it changed on disk (checked once a second).
    pub fn tick(&self) {
        if self.isHidden() {
            return;
        }
        let Some(web) = self.ivars().web.get() else { return };
        let page = unsafe { web.title() }.map(|t| t.to_string()).filter(|t| !t.trim().is_empty());
        let host = unsafe { web.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string());
        if let Some(title) = header_title(page.as_deref(), host.as_deref()) {
            if *self.ivars().title.borrow() != title {
                if let Some(current) = self.ivars().current.borrow().as_ref() {
                    // The page's own name, without the host the header adds.
                    super::panel_history::retitle(current, page.as_deref().unwrap_or(&title));
                }
                *self.ivars().title.borrow_mut() = title;
                self.setNeedsDisplay(true);
            }
        }
        // The theme switched: a page Kova rendered is drawn again in it (a
        // site follows the app's appearance by itself).
        let theme = crate::theme::generation();
        if self.ivars().theme.replace(theme) != theme {
            self.setNeedsDisplay(true);
            let current = self.ivars().current.borrow().clone();
            match current {
                Some(PanelTarget::Markdown(path)) => self.render_markdown_file(web, &path),
                Some(PanelTarget::Sheet(id)) => self.render_sheet(MainThreadMarker::from(self), web, &id),
                Some(PanelTarget::Question(id)) => self.render_question(MainThreadMarker::from(self), web, &id),
                Some(PanelTarget::History) => {
                    let html = super::panel_history::page();
                    unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
                }
                _ => {}
            }
            return;
        }
        let path = match self.ivars().current.borrow().as_ref() {
            Some(PanelTarget::Markdown(p)) => p.clone(),
            _ => return,
        };
        let now = std::time::Instant::now();
        if self.ivars().last_mtime_check.get().is_some_and(|t| now.duration_since(t).as_millis() < 1000) {
            return;
        }
        self.ivars().last_mtime_check.set(Some(now));
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime != self.ivars().rendered_mtime.get() {
            self.render_markdown_file(web, &path);
        }
    }

    /// Whether `responder` (the window's first responder) is inside the
    /// panel: the web view has the keyboard, so the terminal's key map must
    /// leave the editing keys to it.
    pub fn holds(&self, responder: &AnyObject) -> bool {
        let Some(view) = responder.downcast_ref::<NSView>() else { return false };
        view.isDescendantOf(self)
    }

    fn relayout(&self) {
        if let Some(win) = self.window()
            && let Some(kova) = crate::app::kova_view(&win)
        {
            kova.apply_layout();
        }
    }

    fn render_sheet(&self, mtm: MainThreadMarker, web: &WKWebView, id: &str) {
        let html = match crate::app::sheet_page(mtm, id) {
            Some((sheet, answers, from)) => {
                *self.ivars().title.borrow_mut() = sheet.title.clone();
                let home = std::env::var_os("HOME").map(PathBuf::from);
                crate::decision_sheet::render(&sheet, &answers, &from, tokens::get(), home.as_deref())
            }
            None => crate::decision_sheet::render_closed("Decision", None, tokens::get()),
        };
        // No base URL: the page is `about:blank`, the one origin
        // whose script may post to Kova (`did_receive_script_message`).
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
    }

    fn render_question(&self, mtm: MainThreadMarker, web: &WKWebView, id: &str) {
        let html = match crate::app::question_page(mtm, id) {
            Some(q) => {
                *self.ivars().title.borrow_mut() = format!("Question from {}", q.from);
                crate::decision_sheet::render_question(&q.from, &q.message, &q.buttons, q.input, tokens::get())
            }
            None => crate::decision_sheet::render_closed("Question", None, tokens::get()),
        };
        // `about:blank`, like a sheet: the one origin that may post to Kova.
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
    }

    fn render_markdown_file(&self, web: &WKWebView, path: &Path) {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => format!("Cannot read {}: {}", path.display(), e),
        };
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let html = render_markdown(&source, &title, tokens::get());
        let base = path.parent().map(|d| NSURL::fileURLWithPath(&NSString::from_str(&format!("{}/", d.to_string_lossy()))));
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(&html), base.as_deref()) };
        self.ivars().rendered_mtime.set(std::fs::metadata(path).and_then(|m| m.modified()).ok());
    }

    fn web_view(&self, mtm: MainThreadMarker) -> Retained<WKWebView> {
        if let Some(web) = self.ivars().web.get() {
            return web.clone();
        }
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            // Persistent: logins survive a relaunch (WebKit keeps the store on disk).
            config.setWebsiteDataStore(&WKWebsiteDataStore::defaultDataStore(mtm));
            config.setApplicationNameForUserAgent(Some(&NSString::from_str(USER_AGENT_APP)));
        }
        let web = unsafe { WKWebView::initWithFrame_configuration(mtm.alloc(), self.web_frame(), &config) };
        web.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
        let delegate: Retained<PanelWebDelegate> = {
            let this = PanelWebDelegate::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        };
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web.setInspectable(true);
            // The decision sheet's answers (`did_receive_script_message` checks who posts).
            config.userContentController().addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*delegate),
                &NSString::from_str(crate::decision_sheet::HANDLER),
            );
        }
        self.addSubview(&web);
        let _ = self.ivars().delegate.set(delegate);
        let _ = self.ivars().web.set(web.clone());
        web
    }

    /// The web view's frame: under the header, right of the edge separator.
    fn web_frame(&self) -> CGRect {
        let b = self.bounds();
        CGRect {
            // Right of the whole resize zone: over it the web view would take the drag.
            origin: CGPoint { x: EDGE_W, y: HEADER_H },
            size: CGSize { width: (b.size.width - EDGE_W).max(1.0), height: (b.size.height - HEADER_H).max(1.0) },
        }
    }

    fn layout_web(&self) {
        if let Some(web) = self.ivars().web.get() {
            web.setFrame(self.web_frame());
        }
    }

    fn header(&self) -> HeaderLayout {
        HeaderLayout::new(self.bounds().size.width)
    }

    fn local(&self, event: &NSEvent) -> (f64, f64) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        (p.x, p.y)
    }

    /// A link clicked on a sheet or long question: shown here, the sheet
    /// kept to come back to.
    pub fn open_from_sheet(&self, url: String) {
        let from = self.ivars().current.borrow().clone();
        self.open_target(PanelTarget::Web(url));
        *self.ivars().return_to.borrow_mut() = from;
    }

    /// Back to the sheet a link was opened from: its page in the web view's
    /// history is the `about:blank` it was loaded as.
    fn back_is_sheet(&self, web: &WKWebView) -> bool {
        if self.ivars().return_to.borrow().is_none() {
            return false;
        }
        let list = unsafe { web.backForwardList() };
        let item: Option<Retained<objc2::runtime::AnyObject>> = unsafe { msg_send![&*list, backItem] };
        let Some(item) = item else { return true };
        let url: Option<Retained<NSURL>> = unsafe { msg_send![&*item, URL] };
        is_blank(url.and_then(|u| u.absoluteString()).map(|s| s.to_string()).as_deref())
    }

    fn go_back(&self) {
        let Some(web) = self.ivars().web.get() else { return };
        if self.back_is_sheet(web) {
            if let Some(sheet) = self.ivars().return_to.borrow_mut().take() {
                self.open_target(sheet);
            }
        } else if unsafe { web.canGoBack() } {
            unsafe { web.goBack() };
        }
    }

    fn can_go_back(&self) -> bool {
        self.ivars().web.get().is_some_and(|web| self.back_is_sheet(web) || unsafe { web.canGoBack() })
    }

    /// The page in the default browser: the URL the panel is on now (it may
    /// have navigated), or the file.
    fn open_externally(&self) {
        let current = self.ivars().current.borrow().clone();
        let live = self
            .ivars()
            .web
            .get()
            .and_then(|web| unsafe { web.URL() })
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string())
            .filter(|s| crate::ask::is_panel_url(s));
        let url = match (live, current) {
            (Some(url), _) => NSURL::URLWithString(&NSString::from_str(&url)),
            (None, Some(PanelTarget::Web(url))) => NSURL::URLWithString(&NSString::from_str(&url)),
            (None, Some(PanelTarget::Sheet(_) | PanelTarget::Question(_) | PanelTarget::History)) => None,
            (None, Some(target)) => Some(NSURL::fileURLWithPath(&NSString::from_str(&target.external()))),
            (None, None) => None,
        };
        if let Some(url) = url {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    fn draw(&self) {
        let p = tokens::get();
        let b = self.bounds();
        let w = b.size.width;
        fill_rect(&Rect::new(0.0, 0.0, w, b.size.height), p.ground);
        // The separator down the left edge, and the header's bottom rule.
        fill_rect(&Rect::new(0.0, 0.0, SEP_W, b.size.height), p.border_subtle);
        fill_rect(&Rect::new(SEP_W, HEADER_H - 1.0, w - SEP_W, 1.0), p.border_subtle);
        let header = self.header();
        draw_title(&self.ivars().title.borrow(), &header.title, p.text_primary);
        let hovered = self.ivars().hovered.get();
        let pressed = self.ivars().pressed.get();
        let back_enabled = self.can_go_back();
        let on_history = matches!(self.ivars().current.borrow().as_ref(), Some(PanelTarget::History));
        for (hit, rect, icon) in [
            (PanelHit::History, header.history, Icon::Clock),
            (PanelHit::Back, header.back, Icon::ChevronLeft),
            (PanelHit::External, header.external, Icon::Open),
            (PanelHit::Close, header.close, Icon::X),
        ] {
            let enabled = hit != PanelHit::Back || back_enabled;
            let hot = enabled && (hovered == hit || pressed == hit);
            // The history button stays lit while the history shows.
            let on = hit == PanelHit::History && on_history;
            if hot || on {
                fill_round(&rect, BTN_RADIUS, p.white, if pressed == hit { 0.16 } else { 0.10 });
            }
            let ink = if hot || on { p.text_primary } else { p.text_tertiary };
            stroke_icon(icon, &rect, BTN_ICON, ink, if enabled { 1.0 } else { 0.35 });
        }
    }
}

fn cg(r: &Rect) -> CGRect {
    CGRect { origin: CGPoint { x: r.x, y: r.y }, size: CGSize { width: r.w, height: r.h } }
}

fn fill_rect(r: &Rect, c: [f32; 3]) {
    NSColor::colorWithSRGBRed_green_blue_alpha(c[0] as f64, c[1] as f64, c[2] as f64, 1.0).setFill();
    objc2_app_kit::NSBezierPath::bezierPathWithRect(cg(r)).fill();
}

/// The header's title: Inter 13 medium, one line, cut at the tail.
fn draw_title(text: &str, r: &Rect, c: [f32; 3]) {
    super::fonts::register_inter();
    let font = NSFont::fontWithName_size(&NSString::from_str(super::fonts::inter_name(super::sidebar_view::Weight::Medium)), TITLE_SIZE)
        .unwrap_or_else(|| NSFont::systemFontOfSize(TITLE_SIZE));
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(c[0] as f64, c[1] as f64, c[2] as f64, 1.0);
    let attrs = unsafe {
        NSDictionary::from_slices::<NSString>(
            &[
                objc2_app_kit::NSFontAttributeName,
                objc2_app_kit::NSForegroundColorAttributeName,
                objc2_app_kit::NSParagraphStyleAttributeName,
            ],
            &[&*font as &AnyObject, &*color as &AnyObject, &*style as &AnyObject],
        )
    };
    let ns = NSString::from_str(text);
    let size = unsafe { ns.sizeWithAttributes(Some(&attrs)) };
    let y = r.y + ((r.h - size.height) / 2.0).round();
    unsafe { ns.drawInRect_withAttributes(cg(&Rect::new(r.x, y, r.w, size.height)), Some(&attrs)) };
}

// ---------------------------------------------------------------
// The web view's delegate
// ---------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaSidePanelWebDelegate"]
    pub struct PanelWebDelegate;

    unsafe impl NSObjectProtocol for PanelWebDelegate {}

    unsafe impl WKNavigationDelegate for PanelWebDelegate {
        /// http(s) loads in the panel; a local markdown link is rendered
        /// rather than shown as raw text; another scheme (mailto:, slack:, an
        /// app's own) goes to the system, but only when the user clicked a link
        /// in the main frame: a page cannot launch an app on its own. Kova's
        /// own pages (rendered markdown, on `file:`) run no script; a local
        /// `.html` file runs its own, as in a browser.
        #[unsafe(method(webView:decidePolicyForNavigationAction:preferences:decisionHandler:))]
        fn decide_policy(
            &self,
            web_view: &WKWebView,
            action: &WKNavigationAction,
            preferences: &WKWebpagePreferences,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy, std::ptr::NonNull<WKWebpagePreferences>)>,
        ) {
            let url = unsafe { action.request() }.URL();
            let clicked = unsafe { action.navigationType() } == WKNavigationType::LinkActivated
                && unsafe { action.targetFrame() }.is_none_or(|f| unsafe { f.isMainFrame() });
            let scheme = url.as_deref().and_then(|u| u.scheme()).map(|s| s.to_string().to_ascii_lowercase());
            let panel = panel_of(web_view);
            let on_sheet = panel.as_ref().is_some_and(|p| p.sheet_id().is_some() || p.question_id().is_some());
            let on_history = panel.as_ref().is_some_and(|p| matches!(p.ivars().current.borrow().as_ref(), Some(PanelTarget::History)));
            let policy = match (scheme.as_deref(), url.as_deref()) {
                // An entry of the history page: open it again.
                (Some(super::panel_history::LINK_SCHEME), Some(u)) => {
                    let index = u.resourceSpecifier().and_then(|s| s.to_string().parse::<usize>().ok());
                    if on_history
                        && clicked
                        && let (Some(panel), Some(target)) = (panel.as_ref(), index.and_then(super::panel_history::entry))
                    {
                        panel.open_target(target);
                    }
                    WKNavigationActionPolicy::Cancel
                }
                // A sheet's (or long question's) web link opens in the panel,
                // and Back comes back to the sheet, rendered again from what
                // Kova keeps (the answers given are drafts there).
                (Some("http" | "https"), Some(u)) if on_sheet && clicked => {
                    if let (Some(panel), Some(link)) = (panel.as_ref(), u.absoluteString()) {
                        panel.open_from_sheet(link.to_string());
                    }
                    WKNavigationActionPolicy::Cancel
                }
                (Some("http" | "https" | "about" | "data" | "blob"), _) => WKNavigationActionPolicy::Allow,
                (Some("file"), Some(u)) => {
                    let path = u.path().map(|p| p.to_string()).unwrap_or_default();
                    let kind = file_kind(Path::new(&path)).map(|make| make(PathBuf::new()));
                    // A local HTML file runs its scripts, as in a browser: an
                    // artifact styled by a CDN script (Tailwind) is raw HTML
                    // without them. It still reads only its own folder
                    // (`allowingReadAccessToURL`). Anything else on `file:`
                    // runs none.
                    if !matches!(kind, Some(PanelTarget::Html(_))) {
                        unsafe { preferences.setAllowsContentJavaScript(false) };
                    }
                    let is_md = matches!(kind, Some(PanelTarget::Markdown(_)));
                    if clicked && is_md && Path::new(&path).is_file() {
                        if let Some(panel) = panel_of(web_view) {
                            panel.open_target(PanelTarget::Markdown(PathBuf::from(path)));
                        }
                        WKNavigationActionPolicy::Cancel
                    } else {
                        WKNavigationActionPolicy::Allow
                    }
                }
                (Some(_), Some(u)) => {
                    if clicked {
                        NSWorkspace::sharedWorkspace().openURL(u);
                    } else {
                        log::info!("side panel: blocked a navigation to {:?} nobody clicked", scheme);
                    }
                    WKNavigationActionPolicy::Cancel
                }
                _ => WKNavigationActionPolicy::Allow,
            };
            handler.call((policy, std::ptr::NonNull::from(preferences)));
        }
    }

    unsafe impl WKScriptMessageHandler for PanelWebDelegate {
        /// The decision sheet posting its answers: a draft as they are given,
        /// `done` on Done. Taken only from the panel's own web view, its main
        /// frame, on `about:blank` (the page Kova loads without a base URL),
        /// while the panel shows a sheet; the tick then keeps them to what
        /// that sheet asks (`Sheet::answers_from`).
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive_script_message(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let Some(web) = (unsafe { message.webView() }) else { return };
            let Some(panel) = panel_of(&web) else { return };
            let question = panel.question_id();
            let Some(id) = panel.sheet_id().or(question.clone()) else { return };
            let frame = unsafe { message.frameInfo() };
            let frame_url = unsafe { frame.request() }.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string());
            let page_url = unsafe { web.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string());
            if !unsafe { frame.isMainFrame() } || !is_blank(frame_url.as_deref()) || !is_blank(page_url.as_deref()) {
                log::warn!("side panel: ignored a sheet message from {:?}", frame_url);
                return;
            }
            let body = unsafe { message.body() };
            let Some(text) = body.downcast_ref::<NSString>().map(|s| s.to_string()) else { return };
            if text.len() > 512 * 1024 {
                return;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return };
            let done = value.get("type").and_then(|t| t.as_str()) == Some("done");
            let question = question.is_some();
            SHEET_POSTS.with(|p| p.borrow_mut().push(SheetPost { id, done, question, answers: value }));
        }
    }

    unsafe impl WKUIDelegate for PanelWebDelegate {
        /// A page that opens a window. A plain `target=_blank` link (no
        /// script waiting on it) opens in the panel itself. A `window.open`
        /// (a sign-in with Google, an OAuth consent) gets a real popup window,
        /// built on the configuration WebKit hands over: that is what keeps
        /// `window.opener`, through which the popup reports the login back to
        /// the page. The popup shares the panel's persistent data store.
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            web_view: &WKWebView,
            configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            features: &WKWindowFeatures,
        ) -> *mut WKWebView {
            let mtm = MainThreadMarker::from(self);
            let size = popup_size(
                unsafe { features.width() }.map(|n| n.doubleValue()),
                unsafe { features.height() }.map(|n| n.doubleValue()),
            );
            let is_link = unsafe { action.navigationType() } == WKNavigationType::LinkActivated;
            if is_link && size.is_none() {
                unsafe { web_view.loadRequest(&action.request()) };
                return std::ptr::null_mut();
            }
            let (w, h) = size.unwrap_or(POPUP_DEFAULT);
            open_popup(mtm, self, web_view, configuration, w, h)
        }

        /// The popup closed itself (`window.close()`, the end of a sign-in).
        #[unsafe(method(webViewDidClose:))]
        fn web_view_did_close(&self, web_view: &WKWebView) {
            close_popup(web_view);
        }
    }
);

/// What a sheet page posted: the tick saves it, and answers on `done`.
pub struct SheetPost {
    pub id: String,
    pub done: bool,
    /// Posted by a long question's page: `{"type":"answer","action","text"}`.
    pub question: bool,
    pub answers: serde_json::Value,
}

thread_local! {
    static SHEET_POSTS: RefCell<Vec<SheetPost>> = const { RefCell::new(Vec::new()) };
}

/// The sheet posts since the last call.
pub fn take_sheet_posts() -> Vec<SheetPost> {
    SHEET_POSTS.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// `about:blank`, or no URL yet (a page loaded from a string).
fn is_blank(url: Option<&str>) -> bool {
    url.is_none_or(|u| u == "about:blank" || u.is_empty())
}

/// A popup's size when the page gives none.
const POPUP_DEFAULT: (f64, f64) = (520.0, 680.0);

/// The size a page asked for its popup, kept on screen: `None` when it asked
/// for none (a plain link).
fn popup_size(w: Option<f64>, h: Option<f64>) -> Option<(f64, f64)> {
    if w.is_none() && h.is_none() {
        return None;
    }
    let clamp = |v: Option<f64>, d: f64| v.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(d).clamp(320.0, 1200.0);
    Some((clamp(w, POPUP_DEFAULT.0), clamp(h, POPUP_DEFAULT.1)))
}

thread_local! {
    /// The popups open now, kept alive until they close.
    static POPUPS: RefCell<Vec<(Retained<objc2_app_kit::NSWindow>, Retained<WKWebView>)>> = const { RefCell::new(Vec::new()) };
}

fn open_popup(
    mtm: MainThreadMarker,
    delegate: &PanelWebDelegate,
    opener: &WKWebView,
    configuration: &WKWebViewConfiguration,
    w: f64,
    h: f64,
) -> *mut WKWebView {
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    // Drop the popups the user closed by hand.
    POPUPS.with(|p| p.borrow_mut().retain(|(win, _)| win.isVisible()));
    let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w, height: h } };
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    let web = unsafe { WKWebView::initWithFrame_configuration(mtm.alloc(), frame, configuration) };
    web.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    unsafe {
        web.setNavigationDelegate(Some(ProtocolObject::from_ref(delegate)));
        web.setUIDelegate(Some(ProtocolObject::from_ref(delegate)));
        web.setInspectable(true);
    }
    window.setContentView(Some(&web));
    // Over the window it came from.
    match opener.window() {
        Some(parent) => {
            let pf = parent.frame();
            window.setFrameOrigin(CGPoint {
                x: pf.origin.x + (pf.size.width - w) / 2.0,
                y: pf.origin.y + (pf.size.height - h) / 2.0,
            });
        }
        None => window.center(),
    }
    window.makeKeyAndOrderFront(None);
    let ptr = Retained::as_ptr(&web) as *mut WKWebView;
    POPUPS.with(|p| p.borrow_mut().push((window, web)));
    log::info!("side panel: opened a popup window ({w}x{h})");
    ptr
}

fn close_popup(web: &WKWebView) {
    let found = POPUPS.with(|p| {
        let mut p = p.borrow_mut();
        let i = p.iter().position(|(_, v)| std::ptr::eq(&**v, web))?;
        Some(p.remove(i))
    });
    if let Some((window, _web)) = found {
        window.close();
    }
}

/// The panel a web view sits in.
fn panel_of(web: &WKWebView) -> Option<Retained<SidePanelView>> {
    let parent = unsafe { web.superview() }?;
    parent.downcast::<SidePanelView>().ok()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ouvrir_takes_a_link_or_an_existing_markdown_or_html_file() {
        let none = |_: &Path| false;
        let yes = |_: &Path| true;
        assert_eq!(open_target("https://example.com/a", None, &none), Some(PanelTarget::Web("https://example.com/a".into())));
        assert_eq!(
            open_target("/tmp/x/draft notes.md#P1", None, &yes),
            Some(PanelTarget::Markdown(PathBuf::from("/tmp/x/draft notes.md")))
        );
        assert_eq!(open_target("/tmp/x/page.html", None, &yes), Some(PanelTarget::Html(PathBuf::from("/tmp/x/page.html"))));
        assert_eq!(
            open_target("~/notes/a.md", Some(Path::new("/Users/me")), &yes),
            Some(PanelTarget::Markdown(PathBuf::from("/Users/me/notes/a.md")))
        );
        // Missing, relative, another type, or plain words: nothing to open.
        assert_eq!(open_target("/tmp/x/draft.md", None, &none), None);
        assert_eq!(open_target("artifacts/a.md", None, &yes), None);
        assert_eq!(open_target("/tmp/x/a.zip", None, &yes), None);
        // An image, a PDF, text or JSON a card points at opens too (Robin, 10/11/2026).
        for f in ["/tmp/x/a.pdf", "/tmp/x/shot.PNG", "/tmp/x/out.json", "/tmp/x/run.log"] {
            assert_eq!(open_target(f, None, &yes), Some(PanelTarget::File(PathBuf::from(f))), "{f}");
        }
        for ext in FILE_EXTENSIONS {
            assert!(open_target(&format!("/tmp/x/a.{ext}"), None, &yes).is_some(), "{ext} is listed, so it opens");
        }
        assert_eq!(open_target("call the bank", None, &yes), None);
        assert_eq!(open_target("https://example.com/a b", None, &yes), None);
        assert_eq!(open_target("file:///tmp/x/a.md", None, &yes), Some(PanelTarget::Markdown(PathBuf::from("/tmp/x/a.md"))));
        // The anchor is checked off the file name only: a `#` in a folder stays.
        let seen = std::cell::RefCell::new(Vec::new());
        let record = |p: &Path| {
            seen.borrow_mut().push(p.to_path_buf());
            true
        };
        open_target("/tmp/a#b/c.md", None, &record);
        assert_eq!(seen.borrow()[0], PathBuf::from("/tmp/a#b/c.md"));
    }

    const MIN: f64 = 300.0;

    #[test]
    fn the_panel_takes_its_share_of_what_the_sidebar_leaves() {
        let s = split(1600.0, 280.0, true, true, 0.5, MIN);
        assert!(s.sidebar);
        assert_eq!(s.panel, Some((1600.0 - 660.0, 660.0)));
        assert_eq!((s.term_x, s.term_w), (280.0, 660.0));
    }

    #[test]
    fn a_popup_keeps_the_size_asked_within_the_screen() {
        assert_eq!(popup_size(None, None), None, "a plain link: no popup");
        assert_eq!(popup_size(Some(500.0), Some(600.0)), Some((500.0, 600.0)));
        assert_eq!(popup_size(Some(5000.0), None), Some((1200.0, POPUP_DEFAULT.1)));
        assert_eq!(popup_size(Some(10.0), Some(f64::NAN)), Some((320.0, POPUP_DEFAULT.1)));
    }

    #[test]
    fn without_a_panel_the_split_is_the_sidebar_one() {
        let s = split(1200.0, 280.0, true, false, 0.5, MIN);
        assert_eq!((s.sidebar, s.term_x, s.term_w, s.panel), (true, 280.0, 920.0, None));
        let s = split(1200.0, 280.0, false, false, 0.5, MIN);
        assert_eq!((s.sidebar, s.term_x, s.term_w, s.panel), (false, 0.0, 1200.0, None));
    }

    #[test]
    fn the_terminal_always_keeps_its_minimum() {
        // 0.8 of 1000 would leave the terminal 200: the panel stops at 700.
        let s = split(1000.0, 0.0, false, true, 0.8, MIN);
        assert_eq!(s.panel, Some((300.0, 700.0)));
        assert_eq!(s.term_w, 300.0);
    }

    #[test]
    fn the_sidebar_yields_before_the_panel_in_a_narrow_window() {
        // 280 + 300 + 320 = 900 > 850: the sidebar goes, the panel stays.
        let s = split(850.0, 280.0, true, true, 0.5, MIN);
        assert!(!s.sidebar);
        assert_eq!(s.term_x, 0.0);
        assert_eq!(s.panel.map(|p| p.1), Some(425.0));
        // Too narrow for the panel even alone: both stay as they were.
        let s = split(600.0, 280.0, false, true, 0.5, MIN);
        assert_eq!((s.panel, s.term_w), (None, 600.0));
    }

    #[test]
    fn the_panel_never_goes_under_its_minimum_width() {
        let s = split(1000.0, 0.0, false, true, 0.2, MIN);
        assert_eq!(s.panel.map(|p| p.1), Some(PANEL_MIN_W));
    }

    #[test]
    fn a_drag_stands_for_a_clamped_fraction() {
        assert_eq!(fraction_for(500.0, 1000.0), 0.5);
        assert_eq!(fraction_for(950.0, 1000.0), FRACTION_RANGE.1);
        assert_eq!(fraction_for(10.0, 1000.0), FRACTION_RANGE.0);
        assert_eq!(fraction_for(10.0, 0.0), DEFAULT_FRACTION);
        assert_eq!(clamp_fraction(f32::NAN), DEFAULT_FRACTION);
    }

    #[test]
    fn the_header_buttons_sit_at_the_right_and_the_edge_at_the_left() {
        let h = HeaderLayout::new(600.0);
        assert_eq!(h.close.right(), 600.0 - PAD_RIGHT);
        assert_eq!(h.external.right(), h.close.x - BTN_GAP);
        assert_eq!(h.back.right(), h.external.x - BTN_GAP);
        assert_eq!(h.hit(2.0, 200.0), PanelHit::Edge);
        assert_eq!(h.hit(h.close.x + 4.0, h.close.y + 4.0), PanelHit::Close);
        assert_eq!(h.hit(h.back.x + 4.0, h.back.y + 4.0), PanelHit::Back);
        assert_eq!(h.hit(h.external.x + 4.0, h.external.y + 4.0), PanelHit::External);
        assert_eq!(h.hit(h.history.x + 4.0, h.history.y + 4.0), PanelHit::History);
        assert!(h.history.right() < h.back.x && h.title.right() < h.history.x);
        assert_eq!(h.hit(100.0, 10.0), PanelHit::None);
        assert_eq!(h.hit(h.close.x + 4.0, HEADER_H + 4.0), PanelHit::None);
        assert!(h.title.right() < h.back.x);
    }

    #[test]
    fn links_and_files_resolve_to_what_the_panel_shows() {
        let cwd = Path::new("/work/proj");
        let home = Path::new("/Users/me");
        let all = |_: &Path| true;
        assert_eq!(
            target_for("https://claude.ai/artifact/abc", Some(cwd), Some(home), all),
            Some(PanelTarget::Web("https://claude.ai/artifact/abc".into()))
        );
        assert_eq!(
            target_for("docs/plan.md", Some(cwd), Some(home), all),
            Some(PanelTarget::Markdown("/work/proj/docs/plan.md".into()))
        );
        assert_eq!(
            target_for("~/notes/x.MD", Some(cwd), Some(home), all),
            Some(PanelTarget::Markdown("/Users/me/notes/x.MD".into()))
        );
        assert_eq!(
            target_for("file:///tmp/r.html", None, None, all),
            Some(PanelTarget::Html("/tmp/r.html".into()))
        );
        assert_eq!(
            target_for("src/side_panel.md:12:3", Some(cwd), None, all),
            Some(PanelTarget::Markdown("/work/proj/src/side_panel.md".into()))
        );
        // Not a kind the panel shows, a missing file, a relative path without a cwd.
        assert_eq!(target_for("src/main.rs", Some(cwd), None, all), None);
        assert_eq!(target_for("plan.md", Some(cwd), None, |_| false), None);
        assert_eq!(target_for("plan.md", None, None, all), None);
        assert_eq!(target_for("javascript:alert(1)", Some(cwd), None, all), None);
    }

    #[test]
    fn a_relative_path_is_also_looked_for_in_the_pane_s_ancestors_up_to_home() {
        let home = Path::new("/Users/me");
        let cwd = Path::new("/Users/me/AI/Claap");
        let only = |want: &'static str| move |p: &Path| p == Path::new(want);
        // Printed from the parent folder.
        assert_eq!(
            target_for("Claap/Product/docs/x.md", Some(cwd), Some(home), only("/Users/me/AI/Claap/Product/docs/x.md")),
            Some(PanelTarget::Markdown("/Users/me/AI/Claap/Product/docs/x.md".into()))
        );
        // The pane's own directory wins over an ancestor.
        let both = |p: &Path| p == Path::new("/Users/me/AI/Claap/a.md") || p == Path::new("/Users/me/AI/a.md");
        assert_eq!(target_for("a.md:3", Some(cwd), Some(home), both), Some(PanelTarget::Markdown("/Users/me/AI/Claap/a.md".into())));
        // Then the nearest ancestor, $HOME included, nothing above it.
        assert_eq!(target_for("n.md", Some(cwd), Some(home), only("/Users/me/n.md")), Some(PanelTarget::Markdown("/Users/me/n.md".into())));
        assert_eq!(target_for("n.md", Some(cwd), Some(home), only("/Users/n.md")), None);
        // A pane outside $HOME looks in its own directory only.
        assert_eq!(target_for("n.md", Some(Path::new("/tmp/x")), Some(home), only("/tmp/n.md")), None);
    }

    #[test]
    fn the_header_always_names_the_site() {
        assert_eq!(header_title(Some("Claude"), Some("https://evil.example/x")).as_deref(), Some("Claude · evil.example"));
        assert_eq!(header_title(Some("Claude"), Some("https://claude.ai@evil.example/")).as_deref(), Some("Claude · evil.example"));
        assert_eq!(header_title(None, Some("https://claude.ai/artifact/x")).as_deref(), Some("claude.ai"));
        assert_eq!(header_title(Some("plan.md"), Some("file:///tmp/")).as_deref(), Some("plan.md"));
        assert_eq!(header_title(None, None), None);
    }

    #[test]
    fn markdown_renders_with_raw_html_escaped() {
        let html = render_markdown("# Title\n\n<script>alert(1)</script>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n", "x.md", &tokens::DARK);
        assert!(html.contains("<h1>Title</h1>"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("<table>"));
        assert!(html.contains("color-scheme: dark"));
        let links = render_markdown("[a](javascript:alert(1)) [b](JaVaScript:x) [c](docs/x.md) [d](https://e.f) ![i](https://t.example/p.png) ![j](img/a.png)", "x", &tokens::DARK);
        assert!(!links.to_ascii_lowercase().contains("javascript:"));
        assert!(links.contains("href=\"docs/x.md\"") && links.contains("href=\"https://e.f\""));
        assert!(!links.contains("t.example") && links.contains("src=\"img/a.png\""));
        let light = render_markdown("x", "<b>", &tokens::LIGHT);
        assert!(light.contains("color-scheme: light"));
        assert!(light.contains("<title>&lt;b&gt;</title>"));
    }
}
