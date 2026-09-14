use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub colors: ColorsConfig,
    pub window: WindowConfig,
    pub terminal: TerminalConfig,
    pub status_bar: StatusBarConfig,
    pub tab_bar: TabBarConfig,
    pub splits: SplitsConfig,
    pub global_status_bar: GlobalStatusBarConfig,
    pub layout: LayoutConfig,
    pub keys: KeysConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    pub foreground: [f32; 3],
    pub background: [f32; 3],
    pub cursor: [f32; 3],
    /// The body of a tagged block (see `terminal::paste_block`): the part of an
    /// answer meant to leave the terminal rather than be read in it.
    pub paste_block: [f32; 3],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub columns: u16,
    pub rows: u16,
    pub scrollback: usize,
    pub fps: u32,
    pub cursor_blink_frames: u32,
    pub scroll_sensitivity: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StatusBarConfig {
    pub enabled: bool,
    pub bg_color: [f32; 3],
    pub fg_color: [f32; 3],
    pub cwd_color: [f32; 3],
    pub branch_color: [f32; 3],
    pub scroll_color: [f32; 3],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GlobalStatusBarConfig {
    pub bg_color: [f32; 3],
    pub fg_color: [f32; 3],
    pub time_color: [f32; 3],
    pub scroll_indicator_color: [f32; 3],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SplitsConfig {
    pub min_width: f32,
    /// How much an unfocused pane is faded, 0.0 (not at all) .. 1.0 (invisible).
    pub dim_opacity: f32,
    /// What the fade applies to: the whole pane, or only its text.
    pub dim_mode: DimMode,
    /// Thickness in points of the outline drawn around the focused pane.
    /// 0.0 disables it.
    pub focus_border_width: f32,
    pub focus_border_color: [f32; 3],
}

/// How an unfocused pane is faded. `Full` lays a veil over everything, which
/// also washes out the colours a TUI painted; `Text` leaves every background
/// alone and only fades the glyphs, so a colourful pane stays readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DimMode {
    Full,
    Text,
}

impl Default for SplitsConfig {
    fn default() -> Self {
        SplitsConfig {
            min_width: 300.0,
            dim_opacity: 0.3,
            dim_mode: DimMode::Full,
            focus_border_width: 2.0,
            focus_border_color: [0.4, 0.6, 1.0],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TabBarConfig {
    pub bg_color: [f32; 3],
    pub fg_color: [f32; 3],
    pub active_bg: [f32; 3],
}

/// How the tabs of a window are presented: the strip across the top, or a
/// sticky column down the left listing every tab with its panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutMode {
    Tabs,
    Sidebar,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    pub mode: LayoutMode,
    /// Sidebar width in terminal cells, clamped to `SIDEBAR_WIDTH_RANGE`, so it
    /// keeps the same apparent size across font sizes and displays.
    pub sidebar_width: u16,
    /// New tabs start folded in the sidebar when true.
    pub sidebar_collapsed_default: bool,
}

/// Bounds of `LayoutConfig::sidebar_width`, in cells.
pub const SIDEBAR_WIDTH_RANGE: std::ops::RangeInclusive<u16> = 18..=48;

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            mode: LayoutMode::Tabs,
            sidebar_width: 28,
            sidebar_collapsed_default: false,
        }
    }
}

/// The layout settings Kova changes at runtime (View menu, the sidebar edge
/// drag), kept in their own small file rather than written back into
/// `config.toml`: rewriting the user's TOML would need a comment-preserving
/// editor and is not worth a dependency. A value present here overrides the
/// `[layout]` table on load; absent values leave the table alone.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<LayoutMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u16>,
}

fn prefs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/kova/prefs.json")
}

impl LayoutPrefs {
    /// Read the overrides. A missing or unreadable file is no override at all.
    pub fn load() -> Self {
        let path = prefs_path();
        let Ok(data) = std::fs::read_to_string(&path) else {
            return LayoutPrefs::default();
        };
        match serde_json::from_str(&data) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("Invalid {} ({}); ignoring it", path.display(), e);
                LayoutPrefs::default()
            }
        }
    }

    pub fn save(&self) {
        let path = prefs_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("Failed to create {}: {}", parent.display(), e);
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("Failed to write {}: {}", path.display(), e);
                }
            }
            Err(e) => log::warn!("Failed to serialize layout prefs: {}", e),
        }
    }
}

impl Default for TabBarConfig {
    fn default() -> Self {
        TabBarConfig {
            bg_color: [0.12, 0.12, 0.14],
            fg_color: [0.5, 0.5, 0.55],
            active_bg: [0.22, 0.22, 0.26],
        }
    }
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        StatusBarConfig {
            enabled: true,
            bg_color: [0.15, 0.15, 0.18],
            fg_color: [0.8, 0.8, 0.85],
            cwd_color: [0.6, 0.6, 0.65],
            branch_color: [0.4, 0.7, 0.5],
            scroll_color: [0.8, 0.6, 0.3],
        }
    }
}

impl Default for GlobalStatusBarConfig {
    fn default() -> Self {
        GlobalStatusBarConfig {
            bg_color: [0.10, 0.10, 0.12],
            fg_color: [0.8, 0.8, 0.85],
            time_color: [0.65, 0.65, 0.7],
            scroll_indicator_color: [0.8, 0.6, 0.3],
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font: FontConfig::default(),
            colors: ColorsConfig::default(),
            window: WindowConfig::default(),
            terminal: TerminalConfig::default(),
            status_bar: StatusBarConfig::default(),
            tab_bar: TabBarConfig::default(),
            splits: SplitsConfig::default(),
            global_status_bar: GlobalStatusBarConfig::default(),
            layout: LayoutConfig::default(),
            keys: KeysConfig::default(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: "Hack".to_string(),
            size: 13.0,
        }
    }
}

impl Default for ColorsConfig {
    fn default() -> Self {
        ColorsConfig {
            foreground: [1.0, 1.0, 1.0],
            background: [0.1, 0.1, 0.12],
            cursor: [0.8, 0.8, 0.8],
            paste_block: [0.60, 0.80, 1.0],
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig {
            width: 800.0,
            height: 600.0,
            x: 200.0,
            y: 200.0,
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            columns: 80,
            rows: 24,
            scrollback: 10_000,
            fps: 60,
            cursor_blink_frames: 60,
            scroll_sensitivity: 6.0,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        let mut config = match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<Config>(&content) {
                Ok(mut config) => {
                    log::info!("Loaded config from {}", path.display());
                    config.sanitize();
                    config
                }
                Err(e) => {
                    log::warn!("Invalid config at {}: {}. Using defaults.", path.display(), e);
                    Config::default()
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("Failed to read config at {}: {}", path.display(), e);
                }
                Config::default()
            }
        };
        config.apply_layout_prefs(&LayoutPrefs::load());
        config
    }

    /// Lay the runtime layout overrides over the `[layout]` table.
    pub fn apply_layout_prefs(&mut self, prefs: &LayoutPrefs) {
        if let Some(mode) = prefs.mode {
            self.layout.mode = mode;
        }
        if let Some(width) = prefs.sidebar_width {
            self.layout.sidebar_width = width;
        }
        self.layout.sidebar_width = clamp_sidebar_width(self.layout.sidebar_width);
    }

    /// Clamp fields that would break the app if left at pathological values
    /// (zero rows/cols, negative fps, etc.). Falls back to defaults field-by-field.
    fn sanitize(&mut self) {
        let d = TerminalConfig::default();
        if self.terminal.columns == 0 {
            log::warn!("config: terminal.columns=0, using default {}", d.columns);
            self.terminal.columns = d.columns;
        }
        if self.terminal.rows == 0 {
            log::warn!("config: terminal.rows=0, using default {}", d.rows);
            self.terminal.rows = d.rows;
        }
        if self.terminal.fps == 0 {
            self.terminal.fps = d.fps;
        }
        let ds = SplitsConfig::default();
        if !(0.0..=1.0).contains(&self.splits.dim_opacity) {
            log::warn!("config: splits.dim_opacity out of 0..1, using default {}", ds.dim_opacity);
            self.splits.dim_opacity = ds.dim_opacity;
        }
        if !(0.0..=64.0).contains(&self.splits.focus_border_width) {
            log::warn!("config: splits.focus_border_width out of 0..64, using default {}", ds.focus_border_width);
            self.splits.focus_border_width = ds.focus_border_width;
        }
        if self.font.size <= 0.0 {
            let ds = FontConfig::default().size;
            log::warn!("config: font.size<=0, using default {}", ds);
            self.font.size = ds;
        }
        let width = clamp_sidebar_width(self.layout.sidebar_width);
        if width != self.layout.sidebar_width {
            log::warn!("config: layout.sidebar_width out of {:?}, using {}", SIDEBAR_WIDTH_RANGE, width);
            self.layout.sidebar_width = width;
        }
    }
}

/// Snap a sidebar width (in cells) into `SIDEBAR_WIDTH_RANGE`.
pub fn clamp_sidebar_width(cells: u16) -> u16 {
    cells.clamp(*SIDEBAR_WIDTH_RANGE.start(), *SIDEBAR_WIDTH_RANGE.end())
}


fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/kova/config.toml")
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    pub new_tab: String,
    pub close_pane_or_tab: String,
    pub vsplit: String,
    pub hsplit: String,
    pub vsplit_root: String,
    pub hsplit_root: String,
    pub new_window: String,
    pub close_window: String,
    pub kill_window: String,
    pub copy: String,
    pub copy_raw: String,
    pub paste: String,
    pub toggle_filter: String,
    pub prev_tab: String,
    pub next_tab: String,
    pub rename_tab: String,
    pub rename_pane: String,
    pub detach_tab: String,
    pub break_pane: String,
    pub merge_tab: String,
    pub merge_window: String,

    pub switch_tab_1: String,
    pub switch_tab_2: String,
    pub switch_tab_3: String,
    pub switch_tab_4: String,
    pub switch_tab_5: String,
    pub switch_tab_6: String,
    pub switch_tab_7: String,
    pub switch_tab_8: String,
    pub switch_tab_9: String,
    pub navigate_up: String,
    pub navigate_down: String,
    pub navigate_left: String,
    pub navigate_right: String,
    pub swap_up: String,
    pub swap_down: String,
    pub swap_left: String,
    pub swap_right: String,
    pub reparent_up: String,
    pub reparent_down: String,
    pub reparent_left: String,
    pub reparent_right: String,
    pub resize_left: String,
    pub resize_right: String,
    pub resize_up: String,
    pub resize_down: String,
    pub edge_grow_left: String,
    pub edge_grow_right: String,
    pub minimize_pane: String,
    pub restore_minimized: String,
    pub toggle_help: String,
    pub close_tab: String,
    pub open_recent_project: String,
    pub open_search: String,
    pub open_pane_switcher: String,
    pub open_unread_switcher: String,
    pub toggle_bookmark: String,
    pub equalize: String,
    pub repaint_pane: String,
    pub next_attention: String,
    pub history_back: String,
    pub history_forward: String,
    pub toggle_sidebar: String,
    pub terminal: TerminalKeysConfig,
}

impl Default for KeysConfig {
    fn default() -> Self {
        KeysConfig {
            new_tab: "cmd+t".into(),
            close_pane_or_tab: "cmd+w".into(),
            vsplit: "cmd+d".into(),
            hsplit: "cmd+shift+d".into(),
            vsplit_root: "cmd+e".into(),
            hsplit_root: "cmd+shift+e".into(),
            new_window: "cmd+n".into(),
            close_window: "cmd+q".into(),
            kill_window: "cmd+option+q".into(),
            copy: "cmd+c".into(),
            copy_raw: "cmd+shift+c".into(),
            paste: "cmd+v".into(),
            toggle_filter: "cmd+f".into(),
            prev_tab: "cmd+shift+[".into(),
            next_tab: "cmd+shift+]".into(),
            rename_tab: "cmd+shift+r".into(),
            rename_pane: "cmd+option+r".into(),
            detach_tab: "cmd+shift+t".into(),
            break_pane: "cmd+ctrl+t".into(),
            merge_tab: "cmd+ctrl+m".into(),
            merge_window: "cmd+ctrl+shift+m".into(),

            switch_tab_1: "cmd+1".into(),
            switch_tab_2: "cmd+2".into(),
            switch_tab_3: "cmd+3".into(),
            switch_tab_4: "cmd+4".into(),
            switch_tab_5: "cmd+5".into(),
            switch_tab_6: "cmd+6".into(),
            switch_tab_7: "cmd+7".into(),
            switch_tab_8: "cmd+8".into(),
            switch_tab_9: "cmd+9".into(),
            navigate_up: "cmd+option+up".into(),
            navigate_down: "cmd+option+down".into(),
            navigate_left: "cmd+option+left".into(),
            navigate_right: "cmd+option+right".into(),
            swap_up: "cmd+shift+up".into(),
            swap_down: "cmd+shift+down".into(),
            swap_left: "cmd+shift+left".into(),
            swap_right: "cmd+shift+right".into(),
            reparent_up: "cmd+ctrl+shift+up".into(),
            reparent_down: "cmd+ctrl+shift+down".into(),
            reparent_left: "cmd+ctrl+shift+left".into(),
            reparent_right: "cmd+ctrl+shift+right".into(),
            resize_left: "cmd+ctrl+left".into(),
            resize_right: "cmd+ctrl+right".into(),
            resize_up: "cmd+ctrl+up".into(),
            resize_down: "cmd+ctrl+down".into(),
            edge_grow_left: "cmd+ctrl+option+left".into(),
            edge_grow_right: "cmd+ctrl+option+right".into(),
            minimize_pane: "cmd+m".into(),
            restore_minimized: "cmd+option+m".into(),
            toggle_help: "cmd+shift+/".into(),
            close_tab: "cmd+shift+w".into(),
            open_recent_project: "cmd+o".into(),
            open_search: "cmd+shift+f".into(),
            open_pane_switcher: "cmd+p".into(),
            open_unread_switcher: "cmd+shift+j".into(),
            toggle_bookmark: "cmd+b".into(),
            equalize: "cmd+shift++".into(),
            repaint_pane: "cmd+r".into(),
            next_attention: "cmd+j".into(),
            history_back: "cmd+shift+option+left".into(),
            history_forward: "cmd+shift+option+right".into(),
            toggle_sidebar: "cmd+option+s".into(),
            terminal: TerminalKeysConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TerminalKeysConfig {
    pub kill_line: String,
    pub home: String,
    pub end: String,
    pub word_back: String,
    pub word_forward: String,
    pub shift_enter: String,
}

impl Default for TerminalKeysConfig {
    fn default() -> Self {
        TerminalKeysConfig {
            kill_line: "cmd+backspace".into(),
            home: "cmd+left".into(),
            end: "cmd+right".into(),
            word_back: "option+left".into(),
            word_forward: "option+right".into(),
            shift_enter: "shift+enter".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_prefs_override_the_toml_table_field_by_field() {
        let mut config = Config::default();
        config.layout.sidebar_width = 30;
        config.apply_layout_prefs(&LayoutPrefs { mode: Some(LayoutMode::Sidebar), sidebar_width: None });
        assert_eq!(config.layout.mode, LayoutMode::Sidebar);
        assert_eq!(config.layout.sidebar_width, 30);
        // An out-of-range width in the prefs is snapped, not refused.
        config.apply_layout_prefs(&LayoutPrefs { mode: None, sidebar_width: Some(200) });
        assert_eq!(config.layout.mode, LayoutMode::Sidebar);
        assert_eq!(config.layout.sidebar_width, 48);
    }

    #[test]
    fn layout_table_parses_and_a_bad_width_is_clamped() {
        let mut config: Config = toml::from_str(
            "[layout]\nmode = \"sidebar\"\nsidebar_width = 4\nsidebar_collapsed_default = true\n",
        ).unwrap();
        config.sanitize();
        assert_eq!(config.layout.mode, LayoutMode::Sidebar);
        assert_eq!(config.layout.sidebar_width, 18);
        assert!(config.layout.sidebar_collapsed_default);
        // Absent table: the tab bar, as before.
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.layout.mode, LayoutMode::Tabs);
        assert_eq!(config.keys.toggle_sidebar, "cmd+option+s");
    }

    #[test]
    fn layout_prefs_serialize_only_what_is_set() {
        let json = serde_json::to_string(&LayoutPrefs { mode: Some(LayoutMode::Tabs), sidebar_width: None }).unwrap();
        assert_eq!(json, "{\"mode\":\"tabs\"}");
        let prefs: LayoutPrefs = serde_json::from_str("{\"sidebar_width\": 22}").unwrap();
        assert_eq!(prefs.mode, None);
        assert_eq!(prefs.sidebar_width, Some(22));
    }
}
