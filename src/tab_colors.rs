//! The tab color palette, shared by the renderer (which paints it) and the IPC
//! transport (which validates `set-tab-color` indices against its length).
//! Kept apart from both so the transport does not depend on the renderer.

/// Predefined tab color palette (macOS Finder-style tags).
/// Each entry is [R, G, B] in 0.0–1.0.
/// The first six are the original palette and keep their indices: saved
/// sessions store the index.
pub const TAB_COLORS: [[f32; 3]; 12] = [
    [0.82, 0.22, 0.22], // Red
    [0.90, 0.55, 0.15], // Orange
    [0.85, 0.75, 0.15], // Yellow
    [0.30, 0.70, 0.30], // Green
    [0.25, 0.50, 0.85], // Blue
    [0.60, 0.35, 0.75], // Violet
    [0.88, 0.32, 0.60], // Pink
    [0.93, 0.42, 0.36], // Coral
    [0.62, 0.78, 0.25], // Lime
    [0.15, 0.68, 0.62], // Teal
    [0.20, 0.72, 0.85], // Cyan
    [0.42, 0.45, 0.85], // Indigo
];

/// The colours' names, in `TAB_COLORS` order, for the colour menus.
pub const TAB_COLOR_NAMES: [&str; 12] = [
    "Red", "Orange", "Yellow", "Green", "Blue", "Violet", "Pink", "Coral", "Lime", "Teal", "Cyan", "Indigo",
];
