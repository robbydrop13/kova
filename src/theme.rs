//! Light, dark or the system's: the appearance of the whole app.
//!
//! Dark is the terminal of `config.toml` (`[colors]` and the bars), as Kova
//! always drew it. Light swaps in a palette picked from the design system's
//! light tokens (`window::ds_tokens::light`), every ANSI slot at 4.5:1 or more
//! on its white ground. System follows macOS, live: the app timer asks AppKit
//! once a second (`app::poll_system_theme`).
//!
//! The choice is one setting for the whole app, kept in `prefs.json` with the
//! layout. What is resolved from it (light or not) is process-wide, not
//! thread-local like the sidebar's palette: the PTY threads read it too, when
//! a program prints an ANSI colour (`AnsiColor::to_rgb`).
//!
//! A terminal stores resolved RGB in its cells, so a switch also recolours
//! what is already on screen and in the scrollback (`TerminalState::recolor`).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    /// The default: what Kova was before the setting existed.
    #[default]
    Dark,
    System,
}

impl ThemeMode {
    /// The shortcut's cycle: Clair, Sombre, Système, and round again.
    pub fn next(self) -> Self {
        match self {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::System,
            ThemeMode::System => ThemeMode::Light,
        }
    }

    /// Whether this mode draws light, macOS being dark or not.
    pub fn is_light(self, system_dark: bool) -> bool {
        match self {
            ThemeMode::Light => true,
            ThemeMode::Dark => false,
            ThemeMode::System => !system_dark,
        }
    }

    /// The View menu's item for this mode.
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Light => "Clair",
            ThemeMode::Dark => "Sombre",
            ThemeMode::System => "Système",
        }
    }

    /// Tag of the View menu item (`setTheme:` reads it back).
    pub fn tag(self) -> isize {
        match self {
            ThemeMode::Light => 0,
            ThemeMode::Dark => 1,
            ThemeMode::System => 2,
        }
    }

    pub fn from_tag(tag: isize) -> Option<Self> {
        [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System].into_iter().find(|m| m.tag() == tag)
    }
}

static MODE: AtomicU8 = AtomicU8::new(1);
static LIGHT: AtomicBool = AtomicBool::new(false);
/// Bumped on every switch of `LIGHT`: a web view compares it with the one it
/// loaded under and renders again when it moved.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn mode_to_u8(mode: ThemeMode) -> u8 {
    mode.tag() as u8
}

pub fn mode() -> ThemeMode {
    ThemeMode::from_tag(MODE.load(Ordering::Relaxed) as isize).unwrap_or_default()
}

/// Whether the app draws light right now.
pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Take `mode` with macOS dark or not. Returns whether the app went from
/// light to dark or back, i.e. whether every surface has to be redrawn.
pub fn set(mode: ThemeMode, system_dark: bool) -> bool {
    MODE.store(mode_to_u8(mode), Ordering::Relaxed);
    let light = mode.is_light(system_dark);
    let changed = LIGHT.swap(light, Ordering::Relaxed) != light;
    if changed {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    changed
}

// ---------------------------------------------------------------
// The terminal palettes
// ---------------------------------------------------------------

/// The 16 ANSI colours of the dark terminal, as Kova always had them.
pub const DARK_ANSI: [[u8; 3]; 16] = [
    [0, 0, 0],
    [204, 51, 51],
    [51, 204, 51],
    [204, 204, 51],
    [77, 77, 230],
    [204, 51, 204],
    [51, 204, 204],
    [191, 191, 191],
    [102, 102, 102],
    [255, 77, 77],
    [77, 255, 77],
    [255, 255, 77],
    [128, 128, 255],
    [255, 77, 255],
    [77, 255, 255],
    [255, 255, 255],
];

/// The light terminal's ANSI colours. Each hue comes from its design-system
/// token (crit, ok, warn, accent, violet, cyan), darkened just enough to read
/// at 4.5:1 on white; "bright" is the more saturated one, never the lighter.
/// Black is the foreground, so a program printing black stays readable;
/// white and bright white keep their usual values, the two slots programs use
/// as a ground rather than as text.
pub const LIGHT_ANSI: [[u8; 3]; 16] = [
    [36, 36, 36],
    [169, 22, 10],
    [0, 104, 40],
    [125, 80, 0],
    [40, 80, 220],
    [112, 56, 208],
    [0, 106, 116],
    [191, 191, 191],
    [110, 110, 110],
    [200, 40, 28],
    [0, 128, 48],
    [150, 104, 0],
    [58, 94, 237],
    [124, 77, 235],
    [0, 118, 128],
    [255, 255, 255],
];

pub fn ansi(light: bool) -> &'static [[u8; 3]; 16] {
    if light { &LIGHT_ANSI } else { &DARK_ANSI }
}

const fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

const fn grey(v: u8) -> [f32; 3] {
    rgb(v, v, v)
}

/// The band behind selected text in the light terminal (`SEL_BG`).
pub const LIGHT_SELECTION: [f32; 3] = rgb(213, 225, 255);
/// The dark terminal's selection band.
pub const DARK_SELECTION: [f32; 3] = [0.45, 0.42, 0.20];

/// The selection band of the active theme.
pub fn selection_color() -> [f32; 3] {
    if is_light() { LIGHT_SELECTION } else { DARK_SELECTION }
}

/// `config` as the active theme draws it: unchanged in dark, the light
/// palette over its colours in light.
pub fn themed(config: &Config) -> Config {
    let mut c = config.clone();
    if is_light() {
        apply_light(&mut c);
    }
    c
}

/// Put the light palette over a config's colours.
pub fn apply_light(c: &mut Config) {
    let t1 = grey(36);
    c.colors.foreground = t1;
    c.colors.background = grey(255);
    c.colors.cursor = t1;
    c.colors.paste_block = rgb(35, 59, 152);
    c.status_bar.bg_color = grey(243);
    c.status_bar.fg_color = grey(82);
    c.status_bar.cwd_color = grey(95);
    c.status_bar.branch_color = rgb(0, 86, 30);
    c.status_bar.scroll_color = rgb(125, 80, 0);
    c.tab_bar.bg_color = grey(243);
    c.tab_bar.fg_color = grey(95);
    c.tab_bar.active_bg = grey(255);
    c.global_status_bar.bg_color = grey(250);
    c.global_status_bar.fg_color = grey(82);
    c.global_status_bar.time_color = grey(95);
    c.global_status_bar.scroll_indicator_color = rgb(125, 80, 0);
    c.switcher.selected_bg = LIGHT_SELECTION;
    // The dark navy band would leave the grey status text unreadable.
    c.bookmarks.bar_bg = LIGHT_SELECTION;
}

/// What a terminal's cells are recoloured to on a switch: the theme's default
/// foreground and background, and its ANSI colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TermColors {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

impl TermColors {
    /// The colours a themed config (`themed`) gives a terminal.
    pub fn of(themed: &Config, light: bool) -> Self {
        TermColors {
            fg: crate::terminal::color_to_u8(themed.colors.foreground),
            bg: crate::terminal::color_to_u8(themed.colors.background),
            ansi: *ansi(light),
        }
    }
}

/// SGR 2: the foreground halved (`TerminalState::effective_colors`).
pub fn dim(c: [u8; 3]) -> [u8; 3] {
    c.map(|v| v / 2)
}

/// SGR 1: the foreground brightened by 30%, clamped.
pub fn bold(c: [u8; 3]) -> [u8; 3] {
    c.map(|v| (v as u16 * 13 / 10).min(255) as u8)
}

/// A colour of a cell in the old palette, in the new one. `fg` says which
/// side of the cell it is: a foreground matches the old default foreground
/// first, a background the old default background first, and each then the
/// other default (reverse video swapped them) before the ANSI slots. Any other
/// colour (256-colour cube, truecolor) is the program's own and stays.
/// A colour written dim, or bold on a `bold` cell, is the old colour through
/// that transform and becomes the new one through it.
pub fn recolor(c: [u8; 3], fg: bool, from: &TermColors, to: &TermColors, bold_cell: bool) -> [u8; 3] {
    let (first, then) = if fg { ((from.fg, to.fg), (from.bg, to.bg)) } else { ((from.bg, to.bg), (from.fg, to.fg)) };
    let pairs = || [first, then].into_iter().chain(from.ansi.iter().copied().zip(to.ansi.iter().copied()));
    let plain = |c: [u8; 3]| c;
    let dim_bold = |c: [u8; 3]| bold(dim(c));
    let transforms: &[&dyn Fn([u8; 3]) -> [u8; 3]] =
        if bold_cell { &[&plain, &bold, &dim_bold] } else { &[&plain, &dim] };
    for t in transforms {
        if let Some((_, new)) = pairs().find(|(old, _)| t(*old) == c) {
            return t(new);
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shortcut_cycles_light_dark_system() {
        assert_eq!(ThemeMode::Light.next(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Dark.next(), ThemeMode::System);
        assert_eq!(ThemeMode::System.next(), ThemeMode::Light);
    }

    #[test]
    fn system_follows_macos_and_the_others_do_not() {
        assert!(ThemeMode::System.is_light(false));
        assert!(!ThemeMode::System.is_light(true));
        assert!(ThemeMode::Light.is_light(true));
        assert!(!ThemeMode::Dark.is_light(false));
    }

    #[test]
    fn menu_tags_round_trip() {
        for m in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System] {
            assert_eq!(ThemeMode::from_tag(m.tag()), Some(m));
        }
        assert_eq!(ThemeMode::from_tag(7), None);
        assert_eq!(ThemeMode::System.label(), "Système");
    }

    #[test]
    fn the_mode_round_trips_through_prefs_json() {
        let prefs = crate::config::LayoutPrefs { theme: Some(ThemeMode::System), ..Default::default() };
        let json = serde_json::to_string(&prefs).unwrap();
        assert!(json.contains("\"theme\":\"system\""), "{json}");
        let back: crate::config::LayoutPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme, Some(ThemeMode::System));
        // A prefs.json from before the setting: dark, as Kova was.
        let old: crate::config::LayoutPrefs = serde_json::from_str(r#"{"mode":"sidebar"}"#).unwrap();
        assert_eq!(old.theme.unwrap_or_default(), ThemeMode::Dark);
    }

    #[test]
    fn every_light_text_colour_reads_on_white() {
        fn lum(c: [u8; 3]) -> f64 {
            let ch = |v: u8| {
                let s = v as f64 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
        }
        // White and bright white (7, 15) are grounds, not text.
        for (i, c) in LIGHT_ANSI.iter().enumerate().filter(|(i, _)| *i != 7 && *i != 15) {
            let ratio = 1.05 / (lum(*c) + 0.05);
            assert!(ratio >= 4.5, "slot {i} {c:?}: {ratio:.2}");
        }
    }

    #[test]
    fn the_light_config_uses_the_light_terminal() {
        let mut c = Config::default();
        apply_light(&mut c);
        let t = TermColors::of(&c, true);
        assert_eq!(t.bg, [255, 255, 255]);
        assert!(crate::window::sidebar::tokens::background_is_light(c.colors.background));
        assert!(!crate::window::sidebar::tokens::background_is_light(Config::default().colors.background));
    }

    /// The chain a switch takes, short of AppKit: the light config sets the
    /// sidebar's palette, which picks the Cockpit's tokens (the chat's: in
    /// `chat_view`).
    #[test]
    fn light_mode_puts_the_cockpit_on_the_light_tokens() {
        use crate::window::sidebar::tokens;
        let mut c = Config::default();
        apply_light(&mut c);
        tokens::set_background(c.colors.background);
        let light = tokens::get() == &tokens::LIGHT;
        let cockpit = crate::cockpit::shell(light, "n");
        // Back to dark: the palette is per thread and the test threads are reused.
        tokens::set_background(Config::default().colors.background);
        assert!(light);
        assert!(cockpit.contains("color-scheme:light"));
        // `T1` light (#242424) and `BG0` light (#fafafa), not the dark ones.
        assert!(cockpit.contains("--t1:#242424;") && cockpit.contains("--bg0:#fafafa;"), "{cockpit}");
        assert!(!cockpit.contains("--t1:#e9e9e9;"));
    }

    #[test]
    fn recolor_maps_defaults_first_then_ansi_and_leaves_the_rest() {
        let dark = TermColors { fg: [255, 255, 255], bg: [25, 25, 30], ansi: DARK_ANSI };
        let light = TermColors { fg: [36, 36, 36], bg: [255, 255, 255], ansi: LIGHT_ANSI };
        // The default foreground is also bright white: as a foreground it is
        // the default, so it becomes the dark text.
        assert_eq!(recolor([255, 255, 255], true, &dark, &light, false), [36, 36, 36]);
        assert_eq!(recolor([25, 25, 30], false, &dark, &light, false), [255, 255, 255]);
        // Reverse video: the default background as a foreground.
        assert_eq!(recolor([25, 25, 30], true, &dark, &light, false), [255, 255, 255]);
        assert_eq!(recolor(DARK_ANSI[1], true, &dark, &light, false), LIGHT_ANSI[1]);
        assert_eq!(recolor([10, 200, 90], true, &dark, &light, false), [10, 200, 90]);
        // Bold and dim text keep their weight in the new palette.
        assert_eq!(recolor(bold(DARK_ANSI[3]), true, &dark, &light, true), bold(LIGHT_ANSI[3]));
        assert_eq!(recolor(dim([255, 255, 255]), true, &dark, &light, false), dim([36, 36, 36]));
        // And back.
        assert_eq!(recolor([36, 36, 36], true, &light, &dark, false), [255, 255, 255]);
        assert_eq!(recolor(LIGHT_ANSI[4], false, &light, &dark, false), DARK_ANSI[4]);
    }
}
