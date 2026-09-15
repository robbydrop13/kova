//! Where `Cmd+J` sends the eye next: the unread panes, walked in the
//! sidebar's order after the focused pane (`sidebar::next_unread`). The
//! visit history walked by `Cmd+Shift+Option+arrows` shares the same jump.

use super::sidebar::{next_unread, SidebarSort};
use super::sidebar_model::pane_order;
use super::*;

/// Focus a pane wherever it lives: find the window holding it, bring that
/// window front and let it switch to the right tab. Also flashes the pane
/// border so the jump is visible when it lands far from where the eye was.
fn focus_pane_in_any_window(pane_id: PaneId) {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let ns_windows = app.windows();
    for i in 0..ns_windows.count() {
        let win = ns_windows.objectAtIndex(i);
        let view = match crate::app::kova_view(&win) {
            Some(v) => v,
            None => continue,
        };
        if view.ipc_focus_pane(pane_id) {
            win.makeKeyAndOrderFront(None);
            // The jump crosses tabs and windows, so a border pulse alone does
            // not say where it landed: name the directory in big over the pane.
            // ~54 frames ≈ 0.9s @ 60fps, held opaque until the last 30 fade.
            let label = view.pane_cwd(pane_id).map(|cwd| {
                let home = std::env::var("HOME").unwrap_or_default();
                let (name, parent) = flash_label_parts(&cwd, &home);
                PaneFlashLabel { name, parent }
            });
            view.set_pane_flash(pane_id, 54, label);
            return;
        }
    }
    log::debug!("focus_pane_in_any_window: pane {} not found", pane_id);
}

/// Walk the pane visit history one step and focus what it lands on. `forward`
/// replays the trail toward the most recent pane; otherwise it goes back
/// toward the older ones. Does nothing at either end of the trail.
pub(super) fn do_history_step(forward: bool) {
    let target = if forward {
        crate::pane_history::forward(&pane_history_state)
    } else {
        crate::pane_history::back(&pane_history_state)
    };
    match target {
        Some(id) => focus_pane_in_any_window(id),
        None => log::debug!(
            "pane history: nothing {} of here",
            if forward { "ahead" } else { "behind" }
        ),
    }
}

/// Where a pane recorded in the visit history stands now: still a valid
/// landing spot, minimized (walked over, never focused), or closed.
pub(super) fn pane_history_state(pane_id: PaneId) -> crate::pane_history::PaneState {
    use crate::pane_history::PaneState;
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let ns_windows = app.windows();
    for i in 0..ns_windows.count() {
        let win = ns_windows.objectAtIndex(i);
        let view = match crate::app::kova_view(&win) {
            Some(v) => v,
            None => continue,
        };
        let tabs = view.ivars().tabs.borrow();
        for tab in tabs.iter() {
            if let Some(pane) = tab.pane(pane_id) {
                return if pane.minimized { PaneState::Hidden } else { PaneState::Focusable };
            }
        }
    }
    PaneState::Gone
}

impl KovaView {
    /// Every pane Cmd+J and the pill can land on, with whether it is unread,
    /// across every window: this window's in the sidebar's display order,
    /// then the other windows' in their tab order. Minimized panes never
    /// (the user folded them). Walking every window costs a few dozen
    /// reads: fine per tick.
    pub(super) fn collect_unread(&self) -> Vec<(PaneId, bool)> {
        let sort = self.ivars().sidebar.borrow().sort;
        let mut out = pane_order(&self.sidebar_tab_facts(), sort);
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let ns_windows = app.windows();
        for i in 0..ns_windows.count() {
            let win = ns_windows.objectAtIndex(i);
            let Some(view) = crate::app::kova_view(&win) else { continue };
            if std::ptr::eq(view as *const KovaView, self as *const KovaView) {
                continue;
            }
            out.extend(pane_order(&view.sidebar_tab_facts(), SidebarSort::Kova));
        }
        out
    }

    /// Jump to the next unread pane (`sidebar::next_unread`): the one after
    /// the focused pane in the unread list, wrapping, across tabs and
    /// windows. Nothing unread anywhere: a status line says so.
    pub(super) fn do_focus_next_attention(&self) {
        let active_tab = self.ivars().active_tab.get();
        let current = {
            let tabs = self.ivars().tabs.borrow();
            tabs.get(active_tab).map(|t| t.focused_pane)
        };
        let order = self.collect_unread();
        match next_unread(&order, current) {
            Some(target) => focus_pane_in_any_window(target),
            None => self.set_transient_status("Nothing to read"),
        }
    }
}
