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
    pub bookmarks: BookmarksConfig,
    pub switcher: SwitcherConfig,
    pub keys: KeysConfig,
    pub agent: AgentConfig,
    pub sidebar: SidebarConfig,
    pub attention: AttentionConfig,
    pub chat: ChatConfig,
}

/// The chat mode's options.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    /// A dictation's text sends the message as soon as it is in the box.
    /// Off: it waits in the box until Enter.
    pub voice_auto_send: bool,
    /// Words voice typing should spell right (names, jargon), sent as the
    /// provider's vocabulary; Claap's back takes them from the workspace.
    pub voice_vocabulary: Vec<String>,
}

/// How Kova rebuilds an agent's command line when it puts a conversation back.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Add `--dangerously-skip-permissions` to every Claude resume line, even
    /// when the command that started the conversation did not carry it.
    ///
    /// The flag normally survives on its own: `resume_command` reuses the flags
    /// of the line that started the session. It is lost when that line was an
    /// alias (`alias cc="claude --dangerously-skip-permissions"`), which Kova
    /// refuses to rewrite because it cannot know what the alias expands to.
    /// This setting says to put it back.
    pub claude_skip_permissions: bool,
    /// The tab where a conversation reopened to receive a late `kova ask`
    /// answer goes, as a new column. Empty (the default): a new tab each time.
    /// The tab is found by its title and created with it when none has it.
    pub ask_resume_tab: String,
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

/// Colors of a bookmarked conversation: the band its row wears in the pane
/// switcher (Cmd+P), and the status bar of the pane that still holds it.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BookmarksConfig {
    /// Status-bar background of a pane holding a bookmarked conversation.
    pub bar_bg: [f32; 3],
    /// Switcher row background of a bookmarked pane that is not selected.
    pub row_bg: [f32; 3],
    /// Same row when the selection is on it.
    pub row_selected_bg: [f32; 3],
    /// Text drawn on that band — dark by default, since the band is light.
    pub row_fg: [f32; 4],
    /// Dimmer text on the same band (process name, minimized rows).
    pub row_dim_fg: [f32; 4],
}

impl Default for BookmarksConfig {
    fn default() -> Self {
        BookmarksConfig {
            // Deep enough to sit under the status bar's usual palette (cwd grey,
            // branch green, scroll amber) without washing it out, and to read as
            // a state rather than an alarm — the bell and completion bars own the
            // loud end of the range.
            bar_bg: [0.05, 0.09, 0.24],
            // A light band, dark text on it. The selected variant is the same hue
            // pushed harder, so selection still reads on a row that already has a
            // background of its own.
            row_bg: [0.69, 0.78, 0.86],
            row_selected_bg: [0.51, 0.64, 0.79],
            row_fg: [0.05, 0.07, 0.12, 1.0],
            row_dim_fg: [0.22, 0.30, 0.42, 1.0],
        }
    }
}

/// The pane switcher (Cmd+P).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SwitcherConfig {
    /// Band under the selected row — the cursor of the list.
    pub selected_bg: [f32; 3],
}

impl Default for SwitcherConfig {
    fn default() -> Self {
        SwitcherConfig { selected_bg: [0.25, 0.35, 0.55] }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SplitsConfig {
    /// Width, in points, no pane is ever laid out under: the tab scrolls
    /// horizontally instead (see `Tab::min_layout_width`).
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
    /// Sidebar width in points, clamped to `SIDEBAR_WIDTH_RANGE`. The AppKit
    /// sidebar is laid out in points, independent of the terminal font.
    pub sidebar_width: u16,
    /// New tabs start folded in the sidebar when true.
    pub sidebar_collapsed_default: bool,
}

/// Bounds of `LayoutConfig::sidebar_width`, in points.
pub const SIDEBAR_WIDTH_RANGE: std::ops::RangeInclusive<u16> = 200..=520;
/// Widths below this are the v2 unit (terminal cells) left in an old
/// `prefs.json` or `config.toml`, converted at about 8 pt per cell.
const LEGACY_CELL_WIDTH_MAX: u16 = 100;

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            mode: LayoutMode::Tabs,
            sidebar_width: 280,
            sidebar_collapsed_default: false,
        }
    }
}

/// What the sidebar reads from outside Kova's own state.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SidebarConfig {
    /// The roster CLI, as argv: the program, then the words that print the
    /// team roster as JSON (`["/usr/local/bin/team", "list", "--json"]`).
    /// Absent or empty hides the Team and Dedicated sections: Kova never
    /// guesses a path and never spawns a roster command nobody asked for.
    pub roster_command: Vec<String>,
}

/// The attention queue the Asks view lists above the questions
/// (`crate::attention`): three argv, never a shell line. Absent `command`
/// turns the whole feature off; an absent `act_command` or `close_command`
/// only leaves out the buttons that need it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    /// Prints the open entries as a JSON array
    /// (`["/usr/local/bin/attn", "list", "--open", "--json"]`).
    pub command: Vec<String>,
    /// Records a click: Kova appends the entry id and the button
    /// (`["/usr/local/bin/attn", "act"]`).
    pub act_command: Vec<String>,
    /// Closes an entry: Kova appends the id, then `--note` and its words
    /// (`["/usr/local/bin/attn", "close", "--by", "me"]`).
    pub close_command: Vec<String>,
    /// Snoozes an entry: Kova appends the id and `--until=<ISO 8601>`.
    /// Absent: `close_command`'s program with `snooze`.
    pub snooze_command: Vec<String>,
    /// Writes the detail of an entry that says nothing (`attn explain`):
    /// Kova appends the id. Absent: `close_command`'s program with `explain`.
    pub explain_command: Vec<String>,
    /// Prints the Cockpit's catch-up band as JSON (`attn catchup --json`,
    /// `crate::catchup`). Absent: `command`'s program with `catchup --json`.
    pub catchup_command: Vec<String>,
    /// Marks the band read, now (`attn seen`). Absent: `command`'s program
    /// with `seen`.
    pub seen_command: Vec<String>,
    /// The folder whose work is personal (`"~/Perso"`): a failed routine,
    /// a permission or an error whose cwd sits under it is scoped Perso in
    /// the Cockpit, anything else Pro. Empty: everything is Pro.
    pub perso_dir: String,
}

impl AttentionConfig {
    pub fn commands(&self) -> crate::attention::Commands {
        crate::attention::Commands {
            list: self.command.clone(),
            act: self.act_command.clone(),
            close: self.close_command.clone(),
            snooze: self.snooze_command.clone(),
            explain: self.explain_command.clone(),
        }
    }

    /// The catch-up band's commands: as given, or the list command's
    /// program with fixed words, so a working `command` is enough.
    pub fn catchup_commands(&self) -> crate::catchup::Commands {
        let or_list = |given: &[String], words: &[&str]| -> Vec<String> {
            if !given.is_empty() {
                return given.to_vec();
            }
            self.command.first().map(|bin| std::iter::once(bin.clone()).chain(words.iter().map(|w| w.to_string())).collect()).unwrap_or_default()
        };
        crate::catchup::Commands { read: or_list(&self.catchup_command, &["catchup", "--json"]), seen: or_list(&self.seen_command, &["seen"]) }
    }

    /// `attn add`, for the Cockpit's recording (`crate::record`): `command`'s
    /// program with `add`.
    pub fn add_command(&self) -> Vec<String> {
        self.command.first().map(|bin| vec![bin.clone(), "add".to_string()]).unwrap_or_default()
    }

    /// `perso_dir` with a leading `~/` expanded, `None` when unset.
    pub fn perso_root(&self) -> Option<std::path::PathBuf> {
        let dir = self.perso_dir.trim();
        if dir.is_empty() {
            return None;
        }
        Some(match dir.strip_prefix("~/") {
            Some(rest) => std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
            None => std::path::PathBuf::from(dir),
        })
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
    /// The side panel's share of the room right of the sidebar (0.2..0.8),
    /// set by dragging its edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_fraction: Option<f32>,
    /// Light, dark or the system's (View menu, `cmd+option+l`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<crate::theme::ThemeMode>,
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
            bookmarks: BookmarksConfig::default(),
            switcher: SwitcherConfig::default(),
            keys: KeysConfig::default(),
            agent: AgentConfig::default(),
            sidebar: SidebarConfig::default(),
            attention: AttentionConfig::default(),
            chat: ChatConfig::default(),
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

/// Snap a sidebar width (in points) into `SIDEBAR_WIDTH_RANGE`; a v2 value
/// in cells is converted first.
pub fn clamp_sidebar_width(width: u16) -> u16 {
    let pts = if width < LEGACY_CELL_WIDTH_MAX { width.saturating_mul(8) } else { width };
    pts.clamp(*SIDEBAR_WIDTH_RANGE.start(), *SIDEBAR_WIDTH_RANGE.end())
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
    pub toggle_anchor: String,
    pub focus_anchor: String,
    pub equalize: String,
    pub repaint_pane: String,
    pub next_attention: String,
    /// Mark the focused pane unread by hand, or read when it is unread.
    pub toggle_unread: String,
    pub history_back: String,
    pub history_forward: String,
    pub toggle_sidebar: String,
    /// Show or hide the side panel (the web view at the right).
    pub toggle_side_panel: String,
    /// The side panel on its history of what it opened.
    pub panel_history: String,
    /// The Cockpit's chat column hidden or shown. Not `cmd+option+c`: Alfred's
    /// clipboard.
    pub toggle_chat: String,
    /// The next appearance (light, dark, system). Not `cmd+option+c`: Alfred's
    /// clipboard.
    pub cycle_theme: String,
    pub toggle_voice: String,
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
            toggle_anchor: "cmd+shift+a".into(),
            focus_anchor: "cmd+a".into(),
            equalize: "cmd+shift++".into(),
            repaint_pane: "cmd+r".into(),
            next_attention: "cmd+j".into(),
            toggle_unread: "cmd+u".into(),
            history_back: "cmd+shift+option+left".into(),
            history_forward: "cmd+shift+option+right".into(),
            toggle_sidebar: "cmd+option+s".into(),
            toggle_side_panel: "cmd+option+b".into(),
            panel_history: "cmd+option+y".into(),
            toggle_chat: "cmd+option+t".into(),
            cycle_theme: "cmd+option+l".into(),
            toggle_voice: "cmd+shift+m".into(),
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
        config.layout.sidebar_width = 300;
        config.apply_layout_prefs(&LayoutPrefs { mode: Some(LayoutMode::Sidebar), sidebar_width: None, panel_fraction: None, theme: None });
        assert_eq!(config.layout.mode, LayoutMode::Sidebar);
        assert_eq!(config.layout.sidebar_width, 300);
        // An out-of-range width in the prefs is snapped, not refused.
        config.apply_layout_prefs(&LayoutPrefs { mode: None, sidebar_width: Some(900), panel_fraction: None, theme: None });
        assert_eq!(config.layout.mode, LayoutMode::Sidebar);
        assert_eq!(config.layout.sidebar_width, 520);
        // A v2 width in cells (a prefs.json from before the AppKit sidebar)
        // is read as about 8 pt per cell.
        config.apply_layout_prefs(&LayoutPrefs { mode: None, sidebar_width: Some(32), panel_fraction: None, theme: None });
        assert_eq!(config.layout.sidebar_width, 256);
        assert_eq!(clamp_sidebar_width(4), 200);
    }

    #[test]
    fn the_roster_command_parses_as_argv_and_defaults_to_none() {
        let config: Config = toml::from_str(
            "[sidebar]\nroster_command = [\"/usr/local/bin/team\", \"list\", \"--json\"]\n",
        ).unwrap();
        assert_eq!(config.sidebar.roster_command, ["/usr/local/bin/team", "list", "--json"]);
        // No `[sidebar]` table, or an empty list: no roster, and nothing run.
        assert!(Config::default().sidebar.roster_command.is_empty());
        let empty: Config = toml::from_str("[sidebar]\nroster_command = []\n").unwrap();
        assert!(empty.sidebar.roster_command.is_empty());
    }

    #[test]
    fn the_attention_commands_parse_as_argv_and_default_to_off() {
        let config: Config = toml::from_str(
            "[attention]\ncommand = [\"/usr/local/bin/attn\", \"list\", \"--open\", \"--json\"]\nact_command = [\"/usr/local/bin/attn\", \"act\"]\n",
        )
        .unwrap();
        assert_eq!(config.attention.command, ["/usr/local/bin/attn", "list", "--open", "--json"]);
        assert_eq!(config.attention.act_command, ["/usr/local/bin/attn", "act"]);
        assert!(config.attention.close_command.is_empty());
        let none = Config::default();
        assert!(none.attention.command.is_empty() && none.attention.act_command.is_empty());
        assert_eq!(none.attention.perso_root(), None);
        let catchup = config.attention.catchup_commands();
        assert_eq!(catchup.read, ["/usr/local/bin/attn", "catchup", "--json"]);
        assert_eq!(catchup.seen, ["/usr/local/bin/attn", "seen"]);
        assert_eq!(none.attention.catchup_commands(), crate::catchup::Commands::default());
        let given: Config = toml::from_str("[attention]\ncommand = [\"/a\"]\ncatchup_command = [\"/b\", \"x\"]\n").unwrap();
        assert_eq!(given.attention.catchup_commands().read, ["/b", "x"]);
        assert_eq!(config.attention.add_command(), ["/usr/local/bin/attn", "add"]);
        assert!(none.attention.add_command().is_empty());
        let perso: Config = toml::from_str("[attention]\nperso_dir = \"~/Perso\"\n").unwrap();
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        assert_eq!(perso.attention.perso_root(), Some(home.join("Perso")));
        let abs: Config = toml::from_str("[attention]\nperso_dir = \"/srv/me\"\n").unwrap();
        assert_eq!(abs.attention.perso_root(), Some(std::path::PathBuf::from("/srv/me")));
    }

    #[test]
    fn layout_table_parses_and_a_bad_width_is_clamped() {
        let mut config: Config = toml::from_str(
            "[layout]\nmode = \"sidebar\"\nsidebar_width = 4\nsidebar_collapsed_default = true\n",
        ).unwrap();
        config.sanitize();
        assert_eq!(config.layout.mode, LayoutMode::Sidebar);
        assert_eq!(config.layout.sidebar_width, 200);
        assert!(config.layout.sidebar_collapsed_default);
        // Absent table: the tab bar, as before.
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.layout.mode, LayoutMode::Tabs);
        assert_eq!(config.keys.toggle_sidebar, "cmd+option+s");
        assert_eq!(config.keys.toggle_unread, "cmd+u");
    }

    #[test]
    fn layout_prefs_serialize_only_what_is_set() {
        let json = serde_json::to_string(&LayoutPrefs { mode: Some(LayoutMode::Tabs), sidebar_width: None, panel_fraction: None, theme: None }).unwrap();
        assert_eq!(json, "{\"mode\":\"tabs\"}");
        let prefs: LayoutPrefs = serde_json::from_str("{\"sidebar_width\": 22}").unwrap();
        assert_eq!(prefs.mode, None);
        assert_eq!(prefs.sidebar_width, Some(22));
    }
}
