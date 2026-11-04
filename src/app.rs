use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSMenu, NSMenuItem, NSWindow};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer};
use objc2_user_notifications::{
    UNNotification, UNNotificationPresentationOptions, UNNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use objc2::runtime::ProtocolObject;
use std::cell::{Cell, OnceCell, RefCell};
use std::ptr::NonNull;

use crate::config::Config;
use crate::ipc::host::{Disposition, IpcHost, PendingWait};
use crate::window;

pub struct AppDelegateIvars {
    pub windows: RefCell<Vec<Retained<NSWindow>>>,
    /// Windows pending dealloc — kept alive one extra timer tick so AppKit
    /// finishes its run-loop work before the Retained is dropped.
    pending_close: RefCell<Vec<Retained<NSWindow>>>,
    /// Session data collected from windows as they close, so we don't lose
    /// their state when they're deallocated before app termination.
    closed_sessions: RefCell<Vec<crate::session::WindowSession>>,
    config: OnceCell<Config>,
    /// Frame counter for periodic session save (every 30s).
    tick_count: Cell<u64>,
    /// Optional session backup number to restore (--session N).
    session_backup: Option<usize>,
    /// IPC command receiver — polled in the timer tick on the main thread.
    ipc_rx: RefCell<Option<std::sync::mpsc::Receiver<crate::ipc::IpcRequest>>>,
    /// `wait-for-completion` requests that haven't fired yet — checked on each tick.
    pending_waits: RefCell<Vec<PendingWait>>,
    /// Last state published to IPC event subscribers — diffed on each tick.
    events: RefCell<crate::events::EventState>,
    /// Whether Kova is the active app. Kept by the two activation delegate
    /// methods rather than polled: the tick has no `MainThreadMarker` to ask
    /// `NSApplication` with, and an edge-driven flag is exact anyway.
    app_active: Cell<bool>,
    /// True while the main thread is inside its own frame work. The IPC
    /// run-loop source checks it and stands down rather than handling a command
    /// re-entrantly, which would break the borrows the frame is holding.
    main_busy: Cell<bool>,
    /// Agents' questions waiting for the user (`kova ask`), shown on the card.
    asks: RefCell<crate::ask::Board>,
    /// The Asks cards' clicks, queued by the sidebar and served by the same
    /// tick as the floating card's answers: the board lives here.
    sidebar_asks: RefCell<crate::window::sidebar_asks::AskQueue>,
}

impl AppDelegateIvars {
    /// Visit the questions waiting for the user, read-only: the needs model
    /// counts them and Next walks the pane each one came from. The board
    /// lives on the delegate, not in a window, so every window reads the
    /// same list here.
    pub fn for_each_waiting_ask(&self, visit: impl FnMut(&crate::ask::Question)) {
        self.asks.borrow().for_each_waiting(visit);
    }

    /// Queue a click from an Asks card. The main loop drains it.
    pub fn queue_ask_action(&self, action: crate::window::sidebar_asks::AskAction) {
        self.sidebar_asks.borrow_mut().push(action);
    }
}

/// Marks the main thread as busy with Kova's own work for as long as it lives,
/// restoring whatever the previous state was on the way out.
struct BusyGuard<'a> {
    cell: &'a Cell<bool>,
    prev: bool,
}

impl<'a> BusyGuard<'a> {
    fn enter(cell: &'a Cell<bool>) -> Self {
        let prev = cell.replace(true);
        Self { cell, prev }
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.cell.set(self.prev);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KovaAppDelegate"]
    #[ivars = AppDelegateIvars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = MainThreadMarker::from(self);
            let config = self.ivars().config.get().expect("config must be set before applicationDidFinishLaunching");
            log::debug!("Config loaded: {}x{} cols/rows, {} scrollback", config.terminal.columns, config.terminal.rows, config.terminal.scrollback);
            window::sidebar::init(&config.layout);
            window::side_panel::init(crate::config::LayoutPrefs::load().panel_fraction);
            window::sidebar::tokens::set_background(config.colors.background);
            crate::team::set_command(&config.sidebar.roster_command);
            setup_menu(mtm, config);

            // Restore session (all windows) or create a single fresh window
            let restored = crate::session::load(self.ivars().session_backup)
                .and_then(|s| crate::session::restore_session(s, config));

            match restored {
                Some(windows) => {
                    log::info!("Restoring {} window(s) from session", windows.len());
                    let mut win_vec = self.ivars().windows.borrow_mut();
                    for (i, rw) in windows.into_iter().enumerate() {
                        let win = window::create_window(mtm, config, rw.tabs, rw.active_tab, rw.deferred_tabs);
                        if let Some(view) = kova_view(&win) {
                            view.set_sidebar_sort(rw.sidebar_sort);
                        }
                        // Restore saved window position if available
                        if let Some((x, y, w, h)) = rw.frame {
                            let frame = objc2_core_foundation::CGRect {
                                origin: objc2_core_foundation::CGPoint { x, y },
                                size: objc2_core_foundation::CGSize { width: w, height: h },
                            };
                            win.setFrame_display(frame, i == 0);
                        }
                        win.makeKeyAndOrderFront(None);
                        win_vec.push(win);
                    }
                }
                None => {
                    let tab = crate::pane::Tab::new(config).expect("failed to create initial tab");
                    let win = window::create_window(mtm, config, vec![tab], 0, Vec::new());
                    win.makeKeyAndOrderFront(None);
                    self.ivars().windows.borrow_mut().push(win);
                }
            }

            let app = NSApplication::sharedApplication(mtm);
            app.activate();
            // Seed the flag: `activate()` above usually makes AppKit call
            // `applicationDidBecomeActive:` right after, but a launch that stays
            // in the background never gets that call.
            self.ivars().app_active.set(app.isActive());

            // Desktop notifications: Kova posts them itself so that clicking one
            // can focus the pane it came from.
            crate::notification::init(ProtocolObject::from_ref(self));

            // Start IPC server (Unix socket for external process control)
            let ipc_rx = crate::ipc::start();
            *self.ivars().ipc_rx.borrow_mut() = Some(ipc_rx);

            // Let a listener thread wake the main thread the moment it queues a
            // request, instead of the request waiting for the next render frame.
            install_ipc_wakeup();
            // Agents' questions asked before a restart come back on the card,
            // and answers not yet delivered resume their delivery.
            self.ivars().asks.borrow_mut().restore();

            // Start global render timer — single timer for all windows
            self.start_global_timer(config.terminal.fps);
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            let mtm = MainThreadMarker::from(self);
            let mut all_procs = Vec::new();
            let windows = self.ivars().windows.borrow();
            for win in windows.iter() {
                if let Some(view) = kova_view(win) {
                    all_procs.extend(view.running_processes());
                }
            }
            drop(windows);

            if !window::confirm_running_processes(mtm, &all_procs, "Do you want to quit Kova?", "Quit") {
                return NSApplicationTerminateReply::TerminateCancel;
            }
            NSApplicationTerminateReply::TerminateNow
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(
            &self,
            _sender: &NSApplication,
        ) -> bool {
            true
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            self.ivars().app_active.set(true);
        }

        #[unsafe(method(applicationDidResignActive:))]
        fn did_resign_active(&self, _notification: &NSNotification) {
            self.ivars().app_active.set(false);
        }
    }

    impl AppDelegate {
        /// Kova menu → Check for Updates…
        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: &NSMenuItem) {
            crate::update_check::check_now();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            log::info!("Kova shutting down");
            // Save session BEFORE shutting down PTYs (we need them alive for CWD detection)
            // Start with sessions saved when windows were closed during this run
            // (pending_close windows are already in closed_sessions — no need to re-collect)
            let mut window_sessions = self.ivars().closed_sessions.borrow().clone();
            window_sessions.extend(collect_window_sessions(&self.ivars().windows.borrow()));
            crate::session::save(&window_sessions);
            crate::terminal::pty::shutdown_all();
            crate::ipc::cleanup();
            log::logger().flush();
        }
    }

    unsafe impl UNUserNotificationCenterDelegate for AppDelegate {
        /// A notification was clicked. The pane is focused on the next tick —
        /// see `crate::notification::take_pending_focus`.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_notification_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            crate::notification::handle_response(response);
            completion_handler.call(());
        }

        /// Show the banner even when Kova is the frontmost app: the pane that
        /// finished is usually not the one being looked at, so the notification
        /// is still news.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker, config: Config, session_backup: Option<usize>) -> Retained<Self> {
        // `[agent] claude_skip_permissions` applies to every resume line Kova
        // rebuilds, so it is set once here, where the config becomes active.
        crate::claude_session::set_skip_permissions(config.agent.claude_skip_permissions);
        let this = mtm.alloc::<Self>().set_ivars(AppDelegateIvars {
            windows: RefCell::new(Vec::new()),
            pending_close: RefCell::new(Vec::new()),
            tick_count: Cell::new(0),
            closed_sessions: RefCell::new(Vec::new()),
            config: OnceCell::new(),
            session_backup,
            ipc_rx: RefCell::new(None),
            pending_waits: RefCell::new(Vec::new()),
            events: RefCell::new(crate::events::EventState::new()),
            app_active: Cell::new(false),
            main_busy: Cell::new(false),
            asks: RefCell::new(crate::ask::Board::new()),
            sidebar_asks: RefCell::new(crate::window::sidebar_asks::AskQueue::default()),
        });
        let retained: Retained<Self> = unsafe { msg_send![super(this), init] };
        retained.ivars().config.set(config).ok();
        retained
    }

    /// Start a single global NSTimer that ticks all windows.
    fn start_global_timer(&self, fps: u32) {
        let ivars = self.ivars() as *const AppDelegateIvars;
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                1.0 / fps as f64,
                true,
                &RcBlock::new(move |_timer: NonNull<NSTimer>| {
                    let ivars = &*ivars;

                    // Hold the frame open for its whole duration: the IPC
                    // run-loop source must not serve a command in the middle of
                    // a render, nor while a nested run loop (a modal opened from
                    // this tick) is spinning with borrows outstanding.
                    let _busy = BusyGuard::enter(&ivars.main_busy);

                    // Drop windows pending close from the previous tick.
                    // Deferring by one tick lets AppKit finish run-loop work
                    // before the NSWindow/KovaView is deallocated.
                    ivars.pending_close.borrow_mut().clear();


                    // The need list scans every window's panes: build it once
                    // per tick and let each window read the same one.
                    {
                        let windows = ivars.windows.borrow();
                        if let Some(view) = windows.iter().find_map(|win| kova_view(win)) {
                            view.refresh_needs_shared();
                        }
                    }

                    // Tick all windows, collect indices of dead ones
                    let mut dead_indices: Vec<usize> = Vec::new();
                    {
                        let windows = ivars.windows.borrow();
                        for (i, win) in windows.iter().enumerate() {
                            if let Some(view) = kova_view(win) {
                                if !view.tick() {
                                    dead_indices.push(i);
                                }
                            }
                        }
                    }

                    // Resolve any pending `wait-for-completion` requests first,
                    // so a command that just finished is reported on this tick.
                    crate::ipc::host::poll_pending_waits(
                        &KovaAppHost { windows: &ivars.windows },
                        &mut ivars.pending_waits.borrow_mut(),
                    );

                    // Drain any pending search-palette worker results, and bring
                    // an open pane switcher's dots up to date, so the UI
                    // updates without the user pressing a key.
                    {
                        let windows = ivars.windows.borrow();
                        for win in windows.iter() {
                            if let Some(view) = kova_view(win) {
                                view.poll_search_palette();
                                view.poll_pane_switcher();
                            }
                        }
                    }

                    // Serve whatever the IPC listener threads have queued.
                    // The run-loop source below normally gets there first; this
                    // is the belt-and-braces path for a signal that was missed.
                    drain_ipc(ivars, fps);

                    // Answers given on the question card, then the card itself.
                    {
                        let mut asks = ivars.asks.borrow_mut();
                        let queued = ivars.sidebar_asks.borrow_mut().drain();
                        for action in queued {
                            use crate::window::sidebar_asks::AskAction;
                            match action {
                                AskAction::Answer { id, action, text } => {
                                    asks.answer(&id, action, text);
                                }
                                AskAction::Snooze { id, until } => {
                                    if let Err(e) = asks.snooze(&id, until) {
                                        log::warn!("ask: cannot put {} off: {}", id, e);
                                    }
                                }
                                AskAction::Unsnooze { id } => {
                                    if let Err(e) = asks.unsnooze(&id) {
                                        log::warn!("ask: cannot bring {} back: {}", id, e);
                                    }
                                }
                            }
                        }
                        for a in crate::ask_card::take_answers() {
                            asks.answer(&a.id, a.action, a.text);
                        }
                        for s in crate::ask_card::take_snoozes() {
                            let until = crate::ask::snooze_until(crate::ask::now_epoch(), s.seconds);
                            if let Err(e) = asks.snooze(&s.id, until) {
                                log::warn!("ask: cannot put {} off: {}", s.id, e);
                            }
                        }
                        asks.refresh_card(MainThreadMarker::new_unchecked());
                    }
                    // Decision sheets opened from the card: the front window's side panel.
                    let opens = crate::ask_card::take_panel_opens();
                    if !opens.is_empty() {
                        {
                            let mut asks = ivars.asks.borrow_mut();
                            for (id, _) in &opens {
                                asks.opened_in_panel(id);
                            }
                            asks.refresh_card(MainThreadMarker::new_unchecked());
                        }
                        for (_, link) in opens {
                            open_in_front_panel(&ivars.windows, window::side_panel::PanelTarget::from_link(&link));
                        }
                    }
                    // Answers posted by a decision sheet in a side panel.
                    let posts = window::side_panel::take_sheet_posts();
                    if !posts.is_empty() {
                        let mut closed = Vec::new();
                        {
                            let mut asks = ivars.asks.borrow_mut();
                            for post in posts {
                                let Some((sheet, _, _)) = asks.sheet(&post.id) else {
                                    closed.push((post.id, "Decision".to_string(), None));
                                    continue;
                                };
                                let answers = sheet.answers_from(&post.answers);
                                let summary = sheet.summary(&answers);
                                asks.save_draft(&post.id, answers);
                                if post.done {
                                    asks.answer(&post.id, crate::ask::DECISION_DONE.to_string(), String::new());
                                    closed.push((post.id, sheet.title.clone(), Some(summary)));
                                }
                            }
                            asks.refresh_card(MainThreadMarker::new_unchecked());
                        }
                        let windows: Vec<_> = ivars.windows.borrow().clone();
                        for (id, title, summary) in &closed {
                            for win in &windows {
                                if let Some(view) = kova_view(win) {
                                    view.side_panel_sheet_closed(id, title, summary.as_deref());
                                }
                            }
                        }
                    }

                    // Focus the panes whose notification was clicked. Done here
                    // rather than in the delegate callback because this is the
                    // point where the main thread is free to borrow the windows.
                    for pane_id in crate::notification::take_pending_focus() {
                        if let crate::ipc::IpcResponse::Error { message } =
                            crate::ipc::host::focus_pane(&KovaAppHost { windows: &ivars.windows }, pane_id)
                        {
                            log::warn!("Notifications: click ignored: {}", message);
                        }
                    }

                    // Publish whatever changed this tick to IPC event subscribers.
                    // Costs one atomic load when nobody is subscribed.
                    ivars.events.borrow_mut().poll(
                        &ivars.windows,
                        ivars.app_active.get(),
                        fps,
                        false,
                    );

                    // Periodic session save (every ~30s) to survive crashes.
                    // Serialization + I/O is offloaded to a thread to avoid frame drops.
                    let count = ivars.tick_count.get() + 1;
                    ivars.tick_count.set(count);
                    if count % (fps as u64 * 30) == 0 {
                        let sessions = collect_window_sessions(&ivars.windows.borrow());
                        if !sessions.is_empty() {
                            std::thread::spawn(move || {
                                crate::session::save_periodic(&sessions);
                            });
                        }
                    }

                    // Daily release check: polled once a minute, first 5s after launch.
                    if count % (fps as u64 * 60) == fps as u64 * 5 {
                        crate::update_check::poll();
                    }
                    crate::update_check::show_pending(&ivars.windows.borrow());

                    if !dead_indices.is_empty() {
                        // Move dead windows to pending_close — they'll be deallocated
                        // at the start of the next tick.
                        let mut windows = ivars.windows.borrow_mut();
                        let mut pending = ivars.pending_close.borrow_mut();
                        let mut closed = ivars.closed_sessions.borrow_mut();
                        for &idx in dead_indices.iter().rev() {
                            let win = windows.remove(idx);
                            // Save session data before the window is deallocated
                            // (skip if killed with Cmd+Shift+Q)
                            if let Some(view) = kova_view(&win) {
                                if !view.skip_session_save() {
                                    view.append_session_data(&mut closed);
                                }
                            }
                            win.orderOut(None);
                            pending.push(win);
                        }
                        drop(closed);
                        let is_empty = windows.is_empty();
                        drop(pending);
                        drop(windows);

                        if is_empty {
                            let mtm = MainThreadMarker::new_unchecked();
                            let app = NSApplication::sharedApplication(mtm);
                            app.terminate(None);
                        }
                    }
                }),
            )
        };
        let run_loop = NSRunLoop::currentRunLoop();
        unsafe { run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes) };
    }
}

/// Serve every IPC request the listener threads have queued.
///
/// Reached from two places: the run-loop source signalled by the listener
/// thread, which is the fast path and answers within the same run-loop pass,
/// and the render tick, which is the fallback for a signal that never arrived.
/// Both run on the main thread and neither can nest inside the other, so the
/// handlers still see the single-threaded world they were written for.
fn drain_ipc(ivars: &AppDelegateIvars, fps: u32) {
    let rx_borrow = ivars.ipc_rx.borrow();
    let Some(ref rx) = *rx_borrow else { return };
    while let Ok((cmd, responder)) = rx.try_recv() {
        // `subscribe` is the one command that needs the event state, so it is
        // served here rather than in `handle_ipc_command`.
        if let crate::ipc::IpcCommand::Subscribe { topics } = cmd {
            let app_active = ivars.app_active.get();
            // Flush what is already pending first: the subscribers that were
            // here before this one must not learn of a change *after* the new
            // client has been handed it as settled state.
            ivars.events.borrow_mut().poll(&ivars.windows, app_active, fps, true);
            let data = crate::events::snapshot(&ivars.windows, app_active, topics);
            let _ = responder.send(crate::ipc::IpcResponse::Ok { data: Some(data) });
            continue;
        }
        // `ask` waits for the user, however long: the responder is kept until
        // the card is answered.
        if let crate::ipc::IpcCommand::Ask(question) = cmd {
            ivars.asks.borrow_mut().add(*question, responder);
            continue;
        }
        // The three commands KovaLink uses to answer a question from the phone.
        // Served here for the same reason as `ask`: the board lives on the
        // delegate, not in a window.
        match cmd {
            crate::ipc::IpcCommand::ListAsks => {
                let data = ivars.asks.borrow().list_json();
                let _ = responder.send(crate::ipc::IpcResponse::Ok { data: Some(data) });
                continue;
            }
            crate::ipc::IpcCommand::AnswerAsk { ref id, ref action, ref text } => {
                let result = ivars.asks.borrow_mut().answer_external(id, action, text);
                let _ = responder.send(match result {
                    Ok(()) => crate::ipc::IpcResponse::Ok { data: None },
                    Err(message) => crate::ipc::IpcResponse::Error { message },
                });
                continue;
            }
            crate::ipc::IpcCommand::WithdrawAsks { ref session_id } => {
                let withdrawn = ivars.asks.borrow_mut().withdraw(session_id);
                let _ = responder
                    .send(crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({ "withdrawn": withdrawn })) });
                continue;
            }
            crate::ipc::IpcCommand::SnoozeAsk { ref id, seconds } => {
                let until = crate::ask::snooze_until(crate::ask::now_epoch(), seconds);
                let result = ivars.asks.borrow_mut().snooze(id, until);
                let _ = responder.send(match result {
                    Ok(()) => crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({"snoozed_until": until})) },
                    Err(message) => crate::ipc::IpcResponse::Error { message },
                });
                continue;
            }
            _ => {}
        }
        match handle_ipc_command(cmd, &ivars.windows, &ivars.config) {
            Disposition::Reply(response) => {
                let _ = responder.send(response);
            }
            Disposition::Pending(wait) => {
                ivars.pending_waits.borrow_mut().push(PendingWait {
                    pane_id: wait.pane_id,
                    response_tx: responder,
                    deadline: wait.deadline,
                });
            }
        }
    }
}

/// The run-loop source fired: a listener thread has queued an IPC request.
///
/// SAFETY / invariants: a source added to the main run loop only ever performs
/// on the main thread, which is what makes `MainThreadMarker::new_unchecked`
/// and the ivar access below sound.
unsafe extern "C-unwind" fn ipc_source_perform(_info: *mut std::ffi::c_void) {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let ivars = app_delegate(mtm).ivars();
    if ivars.main_busy.get() {
        // A frame is in flight and holding borrows. Leave the request in the
        // channel: that same frame drains it before it ends, so the wait is
        // never longer than it used to be.
        return;
    }
    let _busy = BusyGuard::enter(&ivars.main_busy);
    let fps = ivars
        .config
        .get()
        .map(|c| c.terminal.fps)
        // The config is set before the delegate ever runs, so this is only a
        // formality; 60 is what `TerminalConfig::default` uses.
        .unwrap_or(60);
    drain_ipc(ivars, fps);
}

/// The signalling half of the IPC wake-up, kept alive for the life of the
/// process and called from the listener threads.
struct IpcWaker {
    source: objc2_core_foundation::CFRetained<objc2_core_foundation::CFRunLoopSource>,
    run_loop: objc2_core_foundation::CFRetained<objc2_core_foundation::CFRunLoop>,
}

// SAFETY: `CFRunLoopSourceSignal` and `CFRunLoopWakeUp` are documented as safe
// to call from any thread, and `wake` below is the only thing ever done with
// these two objects once the source has been added to the run loop.
unsafe impl Send for IpcWaker {}
unsafe impl Sync for IpcWaker {}

impl IpcWaker {
    fn wake(&self) {
        self.source.signal();
        self.run_loop.wake_up();
    }
}

/// Add the run-loop source that an IPC listener thread signals, and hand the
/// signalling half to `crate::ipc`.
///
/// Without this, a request sits in the channel until the render timer next
/// fires: one frame of latency on an idle app (16 ms at 60 fps), and the full
/// length of any main-thread stall when the app is busy, which is what left a
/// pasted prompt sitting unsent in a Claude Code composer.
fn install_ipc_wakeup() {
    use objc2_core_foundation::{
        kCFRunLoopCommonModes, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext,
    };

    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(ipc_source_perform),
    };
    // SAFETY: the default allocator is accepted, and `context` is a valid
    // pointer to a correctly versioned (0) source context for the call.
    let Some(source) = (unsafe { CFRunLoopSource::new(None, 0, &mut context) }) else {
        log::error!("IPC: could not create the run-loop source, falling back to the render tick");
        return;
    };
    let Some(run_loop) = CFRunLoop::main() else {
        log::error!("IPC: no main run loop, falling back to the render tick");
        return;
    };
    // Common modes so the source is still served while a menu is tracking or a
    // modal sheet is up, exactly like the render timer.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });

    let waker = IpcWaker { source, run_loop };
    // `waker.wake()` rather than the two fields: a closure that touched the
    // fields would capture them one by one and lose the `Send` impl above.
    crate::ipc::set_main_waker(Box::new(move || waker.wake()));
    log::debug!("IPC: run-loop source installed");
}

/// Collect session data from all live windows.
fn collect_window_sessions(windows: &[Retained<NSWindow>]) -> Vec<crate::session::WindowSession> {
    let mut sessions = Vec::new();
    for win in windows.iter() {
        if let Some(view) = kova_view(win) {
            view.append_session_data(&mut sessions);
        }
    }
    sessions
}

/// Get a reference to our AppDelegate from the shared NSApplication.
/// SAFETY: The app delegate is always our AppDelegate (set in main.rs).
pub fn app_delegate(mtm: MainThreadMarker) -> &'static AppDelegate {
    let app = NSApplication::sharedApplication(mtm);
    let delegate = app.delegate().expect("no app delegate");
    unsafe {
        let raw: *const AppDelegate = msg_send![&*delegate, self];
        &*raw
    }
}

/// Create a new empty window and register it.
/// Called from KovaView on Cmd+N.
pub fn create_new_window(mtm: MainThreadMarker) {
    let ad = app_delegate(mtm);
    let config = ad.ivars().config.get().unwrap();
    let tab = crate::pane::Tab::new(config).expect("failed to create tab");
    let win = window::create_window(mtm, config, vec![tab], 0, Vec::new());
    win.makeKeyAndOrderFront(None);
    ad.ivars().windows.borrow_mut().push(win);
}

/// Detach a tab into a new window, offset from the source window.
pub fn detach_tab_to_new_window(
    mtm: MainThreadMarker,
    tab: crate::pane::Tab,
    source_frame: Option<objc2_core_foundation::CGRect>,
) {
    let ad = app_delegate(mtm);
    let config = ad.ivars().config.get().unwrap();
    let win = window::create_window(mtm, config, vec![tab], 0, Vec::new());

    // Offset new window from source (+20x, -20y cascade)
    if let Some(sf) = source_frame {
        use objc2_core_foundation::{CGPoint, CGRect, CGSize};
        let new_frame = CGRect {
            origin: CGPoint {
                x: sf.origin.x + 20.0,
                y: sf.origin.y - 20.0,
            },
            size: CGSize {
                width: sf.size.width,
                height: sf.size.height,
            },
        };
        win.setFrame_display(new_frame, true);
    }

    win.makeKeyAndOrderFront(None);
    ad.ivars().windows.borrow_mut().push(win);
}

/// Info about a window for the "Send Tab to Window" overlay.
pub struct WindowInfo {
    pub label: String,
    /// Index in the app delegate's windows list.
    pub index: usize,
}

/// List other windows (excluding `source`) with their tab summaries.
pub fn list_other_windows(mtm: MainThreadMarker, source: &NSWindow) -> Vec<WindowInfo> {
    let ad = app_delegate(mtm);
    let windows = ad.ivars().windows.borrow();
    let mut result = Vec::new();
    for (i, win) in windows.iter().enumerate() {
        if win.isEqual(Some(source)) {
            continue;
        }
        let label = if let Some(view) = kova_view(win) {
            let names = view.tab_titles();
            if names.len() == 1 {
                names[0].clone()
            } else {
                format!("{} tabs: {}", names.len(), names.join(", "))
            }
        } else {
            format!("Window {}", i + 1)
        };
        result.push(WindowInfo { label, index: i });
    }
    result
}

/// Send a tab to an existing window (by index in the app delegate's window list).
pub fn send_tab_to_window(mtm: MainThreadMarker, tab: crate::pane::Tab, window_index: usize) {
    let ad = app_delegate(mtm);
    let windows = ad.ivars().windows.borrow();
    if let Some(target) = windows.get(window_index) {
        if let Some(view) = kova_view(target) {
            view.append_tabs(vec![tab]);
        }
        target.makeKeyAndOrderFront(None);
    }
}

/// Append several tabs to an existing window (by index in the app delegate's
/// window list), preserving their order. Used by the whole-window merge.
pub fn send_tabs_to_window(mtm: MainThreadMarker, tabs: Vec<crate::pane::Tab>, window_index: usize) {
    let ad = app_delegate(mtm);
    let windows = ad.ivars().windows.borrow();
    if let Some(target) = windows.get(window_index) {
        if let Some(view) = kova_view(target) {
            view.append_tabs(tabs);
        }
        target.makeKeyAndOrderFront(None);
    }
}

/// Cast the window's contentView to our KovaView, or `None` when the window
/// isn't one of ours.
pub fn kova_view(window: &NSWindow) -> Option<&crate::window::KovaView> {
    let cv = window.contentView()?;
    // The content view is a plain container holding the sidebar and the
    // KovaView side by side: look one level down. Ask the runtime before
    // casting. Several call sites walk `NSApplication::windows()`, which
    // lists every window the process owns, AppKit's own panels and tooltip
    // carriers included. Their content view is not ours, and reading another
    // class's memory as our ivars handed out a `tabs` Vec with a null
    // pointer: Cmd+J then called `Tab::for_each_pane` on a null `self` and
    // segfaulted (crash of 2026-08-14, kova 1.9.0).
    let class = <crate::window::KovaView as objc2::ClassType>::class();
    let subviews = cv.subviews();
    let view = subviews.iter().find(|v| v.isKindOfClass(class))?;
    let ptr: *const objc2_app_kit::NSView = &*view;
    Some(unsafe { &*(ptr as *const crate::window::KovaView) })
}

/// Kova.app as an IPC host: every pane and tab of every window, in the order
/// of the window list. A window whose content view is not a `KovaView` is
/// skipped, as everywhere else.
struct KovaAppHost<'a> {
    windows: &'a RefCell<Vec<Retained<NSWindow>>>,
}

impl KovaAppHost<'_> {
    /// The first window's answer to `ask`, in window-list order.
    fn find_map<T>(&self, mut ask: impl FnMut(&crate::window::KovaView) -> Option<T>) -> Option<T> {
        self.windows.borrow().iter().filter_map(|win| kova_view(win)).find_map(|view| ask(view))
    }
}

impl IpcHost for KovaAppHost<'_> {
    fn collect_panes(&self, out: &mut Vec<serde_json::Value>) {
        for (win_idx, win) in self.windows.borrow().iter().enumerate() {
            let view = match kova_view(win) {
                Some(v) => v,
                None => continue,
            };
            let is_key = win.isKeyWindow();
            view.ipc_collect_panes(win_idx, is_key, out);
        }
    }

    fn collect_pane_ids(&self, out: &mut Vec<u32>) {
        for win in self.windows.borrow().iter() {
            if let Some(view) = kova_view(win) {
                view.ipc_collect_pane_ids(out);
            }
        }
    }

    fn send_keys(&self, pane_id: u32, text: &str) -> bool {
        self.find_map(|view| view.ipc_send_keys(pane_id, text).then_some(())).is_some()
    }

    fn dump_pane_text(
        &self,
        pane_id: u32,
        mode: crate::terminal::DumpMode,
        trim: bool,
    ) -> Option<serde_json::Value> {
        self.find_map(|view| view.ipc_dump_pane_text(pane_id, mode, trim))
    }

    fn measure_pane_text(
        &self,
        pane_id: u32,
        mode: crate::terminal::DumpMode,
        trim: bool,
    ) -> Option<(usize, usize)> {
        self.find_map(|view| view.ipc_measure_pane_text(pane_id, mode, trim))
    }

    fn check_completion(&self, pane_id: u32) -> Option<bool> {
        self.find_map(|view| view.ipc_check_completion(pane_id))
    }

    /// Switches tab inside the window, then raises the window and activates the app.
    fn focus_pane(&self, pane_id: u32) -> bool {
        let wins = self.windows.borrow();
        for win in wins.iter() {
            let view = match kova_view(win) {
                Some(v) => v,
                None => continue,
            };
            if view.ipc_focus_pane(pane_id) {
                win.makeKeyAndOrderFront(None);
                let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
                app.activate();
                return true;
            }
        }
        false
    }

    /// Refuses only the sole pane of the sole tab of a window.
    fn close_pane(&self, pane_id: u32) -> Option<bool> {
        self.find_map(|view| view.ipc_close_pane(pane_id))
    }

    fn set_pane_status(&self, pane_id: u32, waiting: bool) -> bool {
        self.find_map(|view| view.ipc_set_pane_status(pane_id, waiting).then_some(())).is_some()
    }

    fn pane_signal(&self, pane_id: u32, signal: crate::ipc::host::PaneSignal) -> bool {
        self.find_map(|view| view.ipc_pane_signal(pane_id, signal).then_some(())).is_some()
    }

    /// Kova posts it itself because it is the only process that can act on the
    /// click — see `crate::notification`.
    fn notify(&self, pane_id: Option<u32>, title: &str, message: &str, sound: bool) -> Result<(), String> {
        crate::notification::post(title, message, pane_id, sound)
    }

    fn collect_tabs(&self, out: &mut Vec<crate::ipc::host::TabInfo>) {
        for (win_idx, win) in self.windows.borrow().iter().enumerate() {
            let view = match kova_view(win) {
                Some(v) => v,
                None => continue,
            };
            let is_key = win.isKeyWindow();
            view.ipc_collect_tabs(win_idx, is_key, out);
        }
    }

    /// Refuses to close the last tab of its window (would terminate the app).
    fn close_tab(&self, tab_id: u32) -> crate::ipc::host::IpcCloseTabResult {
        use crate::ipc::host::IpcCloseTabResult;
        self.find_map(|view| match view.ipc_close_tab(tab_id) {
            IpcCloseTabResult::NotFound => None,
            found => Some(found),
        })
        .unwrap_or(IpcCloseTabResult::NotFound)
    }
}

/// Dispatch an IPC command: the pane commands, `list-tabs` and `close-tab` go
/// through `crate::ipc::host::dispatch` against Kova.app's windows; windows,
/// layout, anchors and config are Kova.app's own (`handle_ipc_command_sync`).
fn handle_ipc_command(
    cmd: crate::ipc::IpcCommand,
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    config_cell: &OnceCell<Config>,
) -> Disposition {
    match crate::ipc::host::dispatch(&KovaAppHost { windows }, cmd) {
        Ok(disposition) => disposition,
        Err(cmd) => Disposition::Reply(handle_ipc_command_sync(cmd, windows, config_cell)),
    }
}

/// Synchronous handler for the commands that are Kova.app's own: windows,
/// tabs other than `list-tabs`/`close-tab`, layout, anchors, config.
fn handle_ipc_command_sync(
    cmd: crate::ipc::IpcCommand,
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    config_cell: &OnceCell<Config>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcCommand;

    match cmd {
        IpcCommand::Split { direction, cmd: command, cwd, target } => {
            handle_ipc_split(windows, config_cell, &direction, command, cwd, target)
        }
        IpcCommand::NewTab { cwd, cmd } => {
            handle_ipc_new_tab(windows, config_cell, cwd, cmd)
        }
        IpcCommand::SetTabTitle { pane_id, title } => {
            handle_ipc_set_tab_title(windows, pane_id, title)
        }
        IpcCommand::SetTabColor { pane_id, color } => {
            handle_ipc_set_tab_color(windows, pane_id, color)
        }
        IpcCommand::MergeTab { source_tab_id, target_tab_id } => {
            handle_ipc_merge_tab(windows, source_tab_id, target_tab_id)
        }
        IpcCommand::MoveTab { tab_id, index } => {
            handle_ipc_move_tab(windows, tab_id, index)
        }
        IpcCommand::SwapPane { pane_id_a, pane_id_b } => {
            handle_ipc_swap_pane(windows, pane_id_a, pane_id_b)
        }
        IpcCommand::ResizePane { pane_id, axis, direction, amount_pct } => {
            handle_ipc_resize_pane(windows, pane_id, &axis, &direction, amount_pct)
        }
        IpcCommand::RenamePane { pane_id, title } => {
            handle_ipc_rename_pane(windows, pane_id, title)
        }
        IpcCommand::DispatchAction { action, pane_id } => {
            handle_ipc_dispatch_action(windows, &action, pane_id)
        }
        IpcCommand::ResumePane(pane_id) => {
            handle_ipc_resume_pane(windows, pane_id)
        }
        IpcCommand::SetPaneUnread { pane_id, unread } => {
            handle_ipc_set_pane_unread(windows, pane_id, unread)
        }
        IpcCommand::MergeWindow { source_window, target_window } => {
            handle_ipc_merge_window(windows, source_window, target_window)
        }
        IpcCommand::SetAnchor { pane_id, cwd, session_id, agent, label } => {
            handle_ipc_set_anchor(windows, pane_id, cwd, session_id, agent, label)
        }
        IpcCommand::ClearAnchors => handle_ipc_clear_anchors(windows),
        IpcCommand::WakePane(pane_id) => handle_ipc_wake_pane(windows, pane_id),
        IpcCommand::ListSubjects => handle_ipc_list_subjects(windows),
        IpcCommand::SetPaneSubject { pane_id, subject } => {
            handle_ipc_set_pane_subject(windows, pane_id, subject)
        }
        IpcCommand::RenameSubject { tab_id, name, new_name } => {
            handle_ipc_rename_subject(windows, tab_id, &name, &new_name)
        }
        IpcCommand::DeleteSubject { tab_id, name } => handle_ipc_delete_subject(windows, tab_id, &name),
        IpcCommand::SetSubjectColor { tab_id, name, color } => {
            handle_ipc_set_subject_color(windows, tab_id, &name, color)
        }
        IpcCommand::ReloadConfig => {
            handle_ipc_reload_config(windows)
        }
        IpcCommand::OpenPanel { target } => handle_ipc_open_panel(windows, &target),
        // Run by `crate::ipc::host::dispatch` before this dispatcher. Reaching
        // here means that routing was lost in a refactor.
        IpcCommand::ListPanes
        | IpcCommand::ClosePaneById(_)
        | IpcCommand::SendKeys { .. }
        | IpcCommand::FocusPane(_)
        | IpcCommand::GetPaneContent { .. }
        | IpcCommand::CountPaneContent { .. }
        | IpcCommand::WaitForCompletion { .. }
        | IpcCommand::ListTabs
        | IpcCommand::CloseTab(_)
        | IpcCommand::SetPaneStatus { .. }
        | IpcCommand::MarkCompleted { .. }
        | IpcCommand::Bell { .. }
        | IpcCommand::MarkStarted { .. }
        | IpcCommand::Notify { .. } => {
            log::error!("IPC: a host command reached Kova.app's own dispatcher");
            crate::ipc::IpcResponse::Error {
                message: "internal: host command was not dispatched".to_string(),
            }
        }
        // Intercepted in the tick, before this dispatcher — it needs the event
        // state, which lives on the delegate. Reaching here means that branch was
        // lost in a refactor.
        IpcCommand::Ask(_)
        | IpcCommand::ListAsks
        | IpcCommand::AnswerAsk { .. }
        | IpcCommand::SnoozeAsk { .. }
        | IpcCommand::WithdrawAsks { .. } => {
            log::error!("IPC: an ask command reached the generic dispatcher");
            crate::ipc::IpcResponse::Error {
                message: "internal: an ask command was not intercepted".to_string(),
            }
        }
        IpcCommand::Subscribe { .. } => {
            log::error!("IPC: subscribe reached the generic dispatcher");
            crate::ipc::IpcResponse::Error {
                message: "internal: subscribe was not intercepted".to_string(),
            }
        }
    }
}

/// IPC: set the custom title of the tab containing `pane_id`.
fn handle_ipc_set_tab_title(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
    title: Option<String>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        if view.ipc_set_tab_title(pane_id, title.clone()) {
            return IpcResponse::Ok { data: None };
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: set the color of the tab holding `pane_id`.
fn handle_ipc_set_tab_color(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
    color: Option<usize>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        if view.ipc_set_tab_color(pane_id, color) {
            return IpcResponse::Ok { data: None };
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: split the focused pane in the key window.
fn handle_ipc_split(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    config_cell: &OnceCell<Config>,
    direction: &str,
    command: Option<String>,
    cwd: Option<String>,
    target: Option<u32>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let config = match config_cell.get() {
        Some(c) => c,
        None => return IpcResponse::Error { message: "config not loaded".to_string() },
    };

    let wins = windows.borrow();

    // A named pane: split next to it, in whichever window and tab hold it,
    // without touching the focus.
    if let Some(target) = target {
        let split_dir = match direction {
            "vertical" => crate::pane::SplitDirection::Vertical,
            _ => crate::pane::SplitDirection::Horizontal,
        };
        for win in wins.iter() {
            let Some(view) = kova_view(win) else { continue };
            match view.ipc_split_at(config, target, split_dir, cwd.as_deref(), command.clone()) {
                None => continue,
                Some(Some(id)) => {
                    return IpcResponse::Ok { data: Some(serde_json::json!({"pane_id": id})) }
                }
                Some(None) => return IpcResponse::Error { message: "split failed".to_string() },
            }
        }
        return IpcResponse::Error { message: format!("pane {} not found", target) };
    }

    // Find the key window (first one, or the one that isKeyWindow)
    let win = wins.iter()
        .find(|w| w.isKeyWindow())
        .or_else(|| wins.first());
    let win = match win {
        Some(w) => w,
        None => return IpcResponse::Error { message: "no window".to_string() },
    };
    let view = match kova_view(win) {
        Some(v) => v,
        None => return IpcResponse::Error { message: "no view".to_string() },
    };

    let split_dir = match direction {
        "vertical" => crate::pane::SplitDirection::Vertical,
        _ => crate::pane::SplitDirection::Horizontal,
    };

    // Determine CWD: explicit param > focused pane's CWD
    let effective_cwd = cwd.or_else(|| view.ipc_focused_cwd());

    let new_pane_id = view.ipc_split(config, split_dir, effective_cwd.as_deref(), command);
    match new_pane_id {
        Some(id) => IpcResponse::Ok {
            data: Some(serde_json::json!({"pane_id": id})),
        },
        None => IpcResponse::Error { message: "split failed".to_string() },
    }
}

/// IPC: create a new tab in the key window.
fn handle_ipc_new_tab(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    config_cell: &OnceCell<Config>,
    cwd: Option<String>,
    cmd: Option<String>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let config = match config_cell.get() {
        Some(c) => c,
        None => return IpcResponse::Error { message: "config not loaded".to_string() },
    };

    let wins = windows.borrow();
    let win = wins.iter()
        .find(|w| w.isKeyWindow())
        .or_else(|| wins.first());
    let win = match win {
        Some(w) => w,
        None => return IpcResponse::Error { message: "no window".to_string() },
    };
    let view = match kova_view(win) {
        Some(v) => v,
        None => return IpcResponse::Error { message: "no view".to_string() },
    };

    match view.ipc_new_tab(config, cwd.as_deref(), cmd) {
        Some((tab_id, pane_id)) => IpcResponse::Ok {
            data: Some(serde_json::json!({"tab_id": tab_id, "pane_id": pane_id})),
        },
        None => IpcResponse::Error { message: "failed to create tab".to_string() },
    }
}

/// IPC: merge a source tab into a target tab.
fn handle_ipc_merge_tab(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    source_tab_id: u32,
    target_tab_id: u32,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;
    use crate::window::IpcMergeTabResult;

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        match view.ipc_merge_tab(source_tab_id, target_tab_id) {
            IpcMergeTabResult::Merged => return IpcResponse::Ok { data: None },
            IpcMergeTabResult::SourceMissing => continue,
            IpcMergeTabResult::TargetMissing => {
                return IpcResponse::Error {
                    message: format!("target tab {} not found in the same window as source tab {}", target_tab_id, source_tab_id),
                };
            }
        }
    }

    IpcResponse::Error { message: format!("source tab {} not found", source_tab_id) }
}

/// IPC: move a tab to a new position among the tabs of its window.
fn handle_ipc_move_tab(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    tab_id: u32,
    index: usize,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;
    use crate::window::IpcMoveTabResult;

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        match view.ipc_move_tab(tab_id, index) {
            IpcMoveTabResult::Moved => return IpcResponse::Ok { data: None },
            IpcMoveTabResult::NotFound => continue,
        }
    }

    IpcResponse::Error { message: format!("tab {} not found", tab_id) }
}

/// IPC: swap two panes.
fn handle_ipc_swap_pane(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id_a: u32,
    pane_id_b: u32,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;
    use crate::window::IpcSwapPaneResult;

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        match view.ipc_swap_pane(pane_id_a, pane_id_b) {
            IpcSwapPaneResult::Swapped => return IpcResponse::Ok { data: None },
            IpcSwapPaneResult::AMissing => continue,
            IpcSwapPaneResult::BMissing => {
                return IpcResponse::Error {
                    message: format!("pane {} not found in the same tab as pane {}", pane_id_b, pane_id_a),
                };
            }
            IpcSwapPaneResult::Failed => {
                return IpcResponse::Error {
                    message: format!("could not swap panes {} and {}", pane_id_a, pane_id_b),
                };
            }
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id_a) }
}

/// IPC: resize the split containing `pane_id`.
fn handle_ipc_resize_pane(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
    axis: &str,
    direction: &str,
    amount_pct: f32,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;
    use crate::pane::SplitAxis;

    let split_axis = match axis {
        "vertical" => SplitAxis::Vertical,
        _ => SplitAxis::Horizontal,
    };
    let grow = direction == "grow";

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        match view.ipc_resize_pane(pane_id, split_axis, grow, amount_pct) {
            Some(true) => return IpcResponse::Ok { data: None },
            Some(false) => {
                return IpcResponse::Error {
                    message: format!("pane {} has no neighbor along {} axis to resize against", pane_id, axis),
                };
            }
            None => continue,
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: rename a pane (set sticky custom title, like OSC 1).
fn handle_ipc_rename_pane(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
    title: Option<String>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let wins = windows.borrow();
    for win in wins.iter() {
        let view = match kova_view(win) {
            Some(v) => v,
            None => continue,
        };
        if view.ipc_rename_pane(pane_id, title.clone()) {
            return IpcResponse::Ok { data: None };
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: run a pane's resume line, as the sidebar's `Resume` button does.
fn handle_ipc_resume_pane(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let wins = windows.borrow();
    for win in wins.iter() {
        let Some(view) = kova_view(win) else { continue };
        match view.ipc_resume_pane(pane_id) {
            Some(true) => return IpcResponse::Ok { data: None },
            Some(false) => {
                return IpcResponse::Error { message: format!("nothing to resume in pane {}", pane_id) }
            }
            None => continue,
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: set a pane's read state directly.
///
/// Scans the windows exactly like `handle_ipc_set_pane_status` and touches
/// nothing but the pane's own flags. Deliberately NOT modelled on
/// `handle_ipc_dispatch_action` just below, which focuses the target pane and
/// raises its window before dispatching: that focus is what makes a pane count
/// as seen, and it would turn a "mark read" into a "mark unread".
fn handle_ipc_set_pane_unread(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
    unread: bool,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let wins = windows.borrow();
    for win in wins.iter() {
        let Some(view) = kova_view(win) else { continue };
        if view.ipc_set_pane_unread(pane_id, unread) {
            return IpcResponse::Ok { data: None };
        }
    }

    IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: put a conversation at the head of the anchors, and repaint the windows
/// that show them. A pane of this Kova is described by its own state; a
/// conversation named from outside is taken as given — Kova has no way to check
/// that a session id still exists, and refusing on a guess would make the
/// command useless the day an agent changes how it stores its transcripts.
fn handle_ipc_set_anchor(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: Option<u32>,
    cwd: Option<String>,
    session_id: Option<String>,
    agent: Option<String>,
    label: Option<String>,
) -> crate::ipc::IpcResponse {
    let from_pane = pane_id.and_then(|id| {
        windows
            .borrow()
            .iter()
            .filter_map(|win| kova_view(win))
            .find_map(|view| view.ipc_pane_as_saved(id as crate::pane::PaneId))
    });
    // A pane that was named and could not be used is an error, never a silent
    // fallback onto `cwd`: anchoring something other than what was asked for,
    // under an `ok: true`, is the answer that costs the most to debug.
    if let (Some(id), None) = (pane_id, &from_pane) {
        return crate::ipc::IpcResponse::Error {
            message: format!("pane {} is not open, or holds neither a conversation nor a directory", id),
        };
    }
    let anchor = match (from_pane, cwd) {
        (Some(saved), _) => saved,
        (None, Some(cwd)) => crate::anchors::from_named(
            cwd,
            session_id,
            match agent.as_deref() {
                Some("claude") => Some(crate::agent_session::Agent::Claude),
                Some("codex") => Some(crate::agent_session::Agent::Codex),
                Some("pi") => Some(crate::agent_session::Agent::Pi),
                Some("omp") => Some(crate::agent_session::Agent::Omp),
                _ => None,
            },
            label,
        ),
        // Unreachable through `parse_command`, which already refuses a request
        // naming neither. Kept as a message that says what is missing, since a
        // "pane 0 not found" would send the reader looking for a pane.
        (None, None) => {
            return crate::ipc::IpcResponse::Error {
                message: "set-anchor needs either \"pane_id\" or \"cwd\"".to_string(),
            }
        }
    };
    // An id Kova would refuse to type into a shell would anchor a bare shell
    // under a conversation's name. Said out loud rather than shipped as an `ok`.
    if anchor.session_id.is_some() && anchor.resume_command().is_none() {
        return crate::ipc::IpcResponse::Error {
            message: "\"session_id\" is not an id this agent can resume".to_string(),
        };
    }
    let label = anchor.label.clone();
    let mut anchors = crate::anchors::load();
    let dropped = crate::anchors::promote(&mut anchors.items, anchor);
    crate::anchors::save(&anchors);
    for win in windows.borrow().iter() {
        if let Some(view) = kova_view(win) {
            view.ipc_refresh_anchors(&anchors);
        }
    }
    log::info!("IPC: anchored {}", label);
    crate::ipc::IpcResponse::Ok {
        data: Some(serde_json::json!({
            "label": label,
            "anchors": anchors.items.len(),
            "dropped": dropped.map(|a| a.label),
        })),
    }
}

/// IPC: empty the anchor list — the outside saying the day is over.
fn handle_ipc_clear_anchors(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
) -> crate::ipc::IpcResponse {
    let mut anchors = crate::anchors::load();
    let removed = anchors.items.len();
    anchors.items.clear();
    crate::anchors::save(&anchors);
    for win in windows.borrow().iter() {
        if let Some(view) = kova_view(win) {
            view.ipc_refresh_anchors(&anchors);
        }
    }
    log::info!("IPC: cleared {} anchor(s)", removed);
    crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({ "removed": removed })) }
}

/// IPC: resume the agent asleep in `pane_id` where it is, focus untouched.
/// `woken` is false when the pane was not asleep (already running, or never
/// restored with an agent): nothing was done.
fn handle_ipc_wake_pane(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
) -> crate::ipc::IpcResponse {
    for win in windows.borrow().iter() {
        if let Some(woken) = kova_view(win).and_then(|view| view.ipc_wake_pane(pane_id)) {
            if woken {
                log::info!("IPC: woke pane {}", pane_id);
            }
            return crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({ "woken": woken })) };
        }
    }
    crate::ipc::IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// IPC: every subject of every window's tabs, window by window.
fn handle_ipc_list_subjects(windows: &RefCell<Vec<Retained<NSWindow>>>) -> crate::ipc::IpcResponse {
    let mut out = Vec::new();
    for (win_idx, win) in windows.borrow().iter().enumerate() {
        if let Some(view) = kova_view(win) {
            view.ipc_collect_subjects(win_idx, &mut out);
        }
    }
    crate::ipc::IpcResponse::Ok { data: Some(serde_json::Value::Array(out)) }
}

/// IPC: file a pane under a subject of its tab, or take it out of its subject.
fn handle_ipc_set_pane_subject(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    pane_id: u32,
    subject: Option<String>,
) -> crate::ipc::IpcResponse {
    for win in windows.borrow().iter() {
        if let Some(data) = kova_view(win).and_then(|view| view.ipc_set_pane_subject(pane_id, subject.clone())) {
            return crate::ipc::IpcResponse::Ok { data: Some(data) };
        }
    }
    crate::ipc::IpcResponse::Error { message: format!("pane {} not found", pane_id) }
}

/// The error of a subject command whose tab or subject was not found anywhere.
fn subject_error(error: crate::window::SubjectError, tab_id: u32, name: &str) -> crate::ipc::IpcResponse {
    let message = match error {
        crate::window::SubjectError::NoTab => format!("tab {} not found", tab_id),
        crate::window::SubjectError::NoSubject => format!("tab {} has no subject \"{}\"", tab_id, name),
    };
    crate::ipc::IpcResponse::Error { message }
}

/// IPC: rename a subject of a tab, merging into a name the tab already has.
fn handle_ipc_rename_subject(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    tab_id: u32,
    name: &str,
    new_name: &str,
) -> crate::ipc::IpcResponse {
    for win in windows.borrow().iter() {
        let Some(view) = kova_view(win) else { continue };
        match view.ipc_rename_subject(tab_id, name, new_name) {
            Ok(merged) => {
                return crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({ "merged": merged })) };
            }
            Err(crate::window::SubjectError::NoTab) => continue,
            Err(e) => return subject_error(e, tab_id, name),
        }
    }
    subject_error(crate::window::SubjectError::NoTab, tab_id, name)
}

/// IPC: delete a subject of a tab; its panes stay open, out of any box.
fn handle_ipc_delete_subject(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    tab_id: u32,
    name: &str,
) -> crate::ipc::IpcResponse {
    for win in windows.borrow().iter() {
        let Some(view) = kova_view(win) else { continue };
        match view.ipc_delete_subject(tab_id, name) {
            Ok(released) => {
                return crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({ "released": released })) };
            }
            Err(crate::window::SubjectError::NoTab) => continue,
            Err(e) => return subject_error(e, tab_id, name),
        }
    }
    subject_error(crate::window::SubjectError::NoTab, tab_id, name)
}

/// IPC: give a subject of a tab a palette color.
fn handle_ipc_set_subject_color(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    tab_id: u32,
    name: &str,
    color: usize,
) -> crate::ipc::IpcResponse {
    for win in windows.borrow().iter() {
        let Some(view) = kova_view(win) else { continue };
        match view.ipc_set_subject_color(tab_id, name, color) {
            Ok(()) => {
                return crate::ipc::IpcResponse::Ok { data: Some(serde_json::json!({ "color": color })) };
            }
            Err(crate::window::SubjectError::NoTab) => continue,
            Err(e) => return subject_error(e, tab_id, name),
        }
    }
    subject_error(crate::window::SubjectError::NoTab, tab_id, name)
}

/// IPC: re-read the config file and push its appearance half into every open
/// window. Answers with what was left alone, since a reload that silently
/// ignores a font change would read as a broken command.
/// Show `target` in the side panel of the window the user is on (the key
/// window, else the main one, else the first Kova window), and bring that
/// window forward: Open is clicked on the floating card, often while another
/// app is in front. Returns whether a window took it.
/// A decision sheet still waiting, for the side panel to render: the sheet,
/// its answers so far, who asks. `None` once it is answered or withdrawn.
pub(crate) fn sheet_page(
    mtm: MainThreadMarker,
    id: &str,
) -> Option<(crate::decision_sheet::Sheet, crate::decision_sheet::SheetAnswers, String)> {
    let delegate = app_delegate(mtm);
    let asks = delegate.ivars().asks.try_borrow().ok()?;
    asks.sheet(id)
}

fn open_in_front_panel(windows: &RefCell<Vec<Retained<NSWindow>>>, target: window::side_panel::PanelTarget) -> bool {
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let app = NSApplication::sharedApplication(mtm);
    let front = app
        .keyWindow()
        .filter(|w| kova_view(w).is_some())
        .or_else(|| app.mainWindow().filter(|w| kova_view(w).is_some()))
        .or_else(|| windows.borrow().iter().find(|w| kova_view(w).is_some()).cloned());
    let Some(win) = front else {
        log::warn!("side panel: no Kova window to open {:?} in", target);
        return false;
    };
    let Some(view) = kova_view(&win) else { return false };
    view.open_in_side_panel(target);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    win.makeKeyAndOrderFront(None);
    true
}

fn handle_ipc_open_panel(windows: &RefCell<Vec<Retained<NSWindow>>>, target: &str) -> crate::ipc::IpcResponse {
    let Some(target) = window::side_panel::target_on_disk(target, None) else {
        return crate::ipc::IpcResponse::Error {
            message: "\"target\" must be an http(s) URL or an existing .md / .html file (absolute or ~/ path)".to_string(),
        };
    };
    if open_in_front_panel(windows, target) {
        crate::ipc::IpcResponse::Ok { data: None }
    } else {
        crate::ipc::IpcResponse::Error { message: "no Kova window is open".to_string() }
    }
}

fn handle_ipc_reload_config(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
) -> crate::ipc::IpcResponse {
    let config = Config::load();
    // The sidebar follows the terminal background, and the roster command is
    // read here too: both are re-picked on a reload, not only on a restart.
    window::sidebar::tokens::set_background(config.colors.background);
    crate::team::set_command(&config.sidebar.roster_command);
    let mut applied = 0usize;
    for win in windows.borrow().iter() {
        if let Some(view) = kova_view(win) {
            view.ipc_apply_config(&config);
            applied += 1;
        }
    }
    log::info!("IPC: config reloaded into {} window(s)", applied);
    crate::ipc::IpcResponse::Ok {
        data: Some(serde_json::json!({
            "windows": applied,
            "needs_restart": ["font", "keys", "terminal", "window"],
        })),
    }
}

/// IPC: trigger any keyboard action by its stable name. With `pane_id`, the
/// owning window is focused first and the action runs there; without it, the
/// action runs against the key window.
fn handle_ipc_dispatch_action(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    action_name: &str,
    pane_id: Option<u32>,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    let action = match crate::keybindings::action_from_ipc_name(action_name) {
        Some(a) => a,
        None => return IpcResponse::Error { message: format!("unknown action: {}", action_name) },
    };

    // Resolve the target window, then drop the window-list borrow before
    // dispatching — some actions (new-window, detach-tab) mutate the list.
    let target_win = {
        let wins = windows.borrow();
        match pane_id {
            Some(pid) => {
                let mut found = None;
                for win in wins.iter() {
                    if let Some(view) = kova_view(win) {
                        if view.ipc_focus_pane(pid) {
                            win.makeKeyAndOrderFront(None);
                            found = Some(win.clone());
                            break;
                        }
                    }
                }
                match found {
                    Some(w) => w,
                    None => return IpcResponse::Error { message: format!("pane {} not found", pid) },
                }
            }
            None => {
                let win = wins.iter().find(|w| w.isKeyWindow()).or_else(|| wins.first());
                match win {
                    Some(w) => w.clone(),
                    None => return IpcResponse::Error { message: "no window".to_string() },
                }
            }
        }
    };

    let view = match kova_view(&target_win) {
        Some(v) => v,
        None => return IpcResponse::Error { message: "no view".to_string() },
    };
    view.dispatch_action(&action);
    IpcResponse::Ok { data: None }
}

/// IPC: merge every tab of `source_window` into `target_window` (by window-list
/// index), then close the now-empty source window.
fn handle_ipc_merge_window(
    windows: &RefCell<Vec<Retained<NSWindow>>>,
    source_window: usize,
    target_window: usize,
) -> crate::ipc::IpcResponse {
    use crate::ipc::IpcResponse;

    // Validate the target exists *before* draining the source, otherwise an
    // invalid target would lose the source's tabs. Clone the source handle and
    // drop the borrow — merge_window_into re-borrows the list internally.
    let source_win = {
        let wins = windows.borrow();
        if wins.get(target_window).is_none() {
            return IpcResponse::Error { message: format!("target window {} not found", target_window) };
        }
        match wins.get(source_window) {
            Some(w) => w.clone(),
            None => return IpcResponse::Error { message: format!("source window {} not found", source_window) },
        }
    };

    let view = match kova_view(&source_win) {
        Some(v) => v,
        None => return IpcResponse::Error { message: "no view for source window".to_string() },
    };
    view.merge_window_into(target_window);
    IpcResponse::Ok { data: None }
}

/// Tags of the View menu's layout items (`setLayoutMode:` reads them).
pub const LAYOUT_MENU_TAG_TABS: isize = 0;
pub const LAYOUT_MENU_TAG_SIDEBAR: isize = 1;

fn setup_menu(mtm: MainThreadMarker, config: &Config) {
    let menu_bar = NSMenu::new(mtm);
    let app_menu_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);

    // No target: the action travels the responder chain up to AppDelegate.
    let update_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("Check for Updates\u{2026}"),
            Some(objc2::sel!(checkForUpdates:)),
            &NSString::from_str(""),
        )
    };
    app_menu.addItem(&update_item);
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));

    // Cmd+Q is handled in KovaView::performKeyEquivalent (close window, not app).
    // Menu item with empty key so it doesn't compete with performKeyEquivalent.
    let quit_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("Close Window"),
            None,
            &NSString::from_str(""),
        )
    };
    app_menu.addItem(&quit_item);

    app_menu_item.setSubmenu(Some(&app_menu));
    menu_bar.addItem(&app_menu_item);

    // View menu: the two layouts as radio items. No target: the action walks
    // the responder chain to the key window's KovaView, which also keeps the
    // check mark current in `validateMenuItem:`. The shortcut is displayed on
    // "Show Sidebar" but handled in `performKeyEquivalent` like every other
    // binding (it toggles); the menu only ever sees it if the view declines.
    let view_menu_item = NSMenuItem::new(mtm);
    let view_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("View"));
    let layout_item = |title: &str, tag: isize, key: Option<&str>| {
        let (key_eq, mask) = key
            .map(menu_key_equivalent)
            .unwrap_or((String::new(), objc2_app_kit::NSEventModifierFlags::empty()));
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(objc2::sel!(setLayoutMode:)),
                &NSString::from_str(&key_eq),
            )
        };
        item.setTag(tag);
        if !key_eq.is_empty() {
            item.setKeyEquivalentModifierMask(mask);
        }
        item
    };
    view_menu.addItem(&layout_item("Show Tab Bar", LAYOUT_MENU_TAG_TABS, None));
    view_menu.addItem(&layout_item("Show Sidebar", LAYOUT_MENU_TAG_SIDEBAR, Some(&config.keys.toggle_sidebar)));
    view_menu.addItem(&NSMenuItem::separatorItem(mtm));
    {
        let (key_eq, mask) = menu_key_equivalent(&config.keys.toggle_side_panel);
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str("Side Panel"),
                Some(objc2::sel!(toggleSidePanelFromMenu:)),
                &NSString::from_str(&key_eq),
            )
        };
        if !key_eq.is_empty() {
            item.setKeyEquivalentModifierMask(mask);
        }
        view_menu.addItem(&item);
    }
    view_menu_item.setSubmenu(Some(&view_menu));
    menu_bar.addItem(&view_menu_item);

    // Pane menu: the pane shortcuts worth a menu entry. Same responder-chain
    // rule as the View menu; the key window's KovaView handles them, and
    // `validateMenuItem:` retitles the unread item to what Cmd+U will do.
    let pane_menu_item = NSMenuItem::new(mtm);
    let pane_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Pane"));
    let pane_item = |title: &str, selector: objc2::runtime::Sel, key: &str| {
        let (key_eq, mask) = menu_key_equivalent(key);
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(selector),
                &NSString::from_str(&key_eq),
            )
        };
        if !key_eq.is_empty() {
            item.setKeyEquivalentModifierMask(mask);
        }
        item
    };
    pane_menu.addItem(&pane_item("Rename Pane\u{2026}", objc2::sel!(renamePaneFromMenu:), &config.keys.rename_pane));
    pane_menu.addItem(&pane_item("Mark as Unread", objc2::sel!(toggleUnreadFromMenu:), &config.keys.toggle_unread));
    pane_menu_item.setSubmenu(Some(&pane_menu));
    menu_bar.addItem(&pane_menu_item);

    let app = NSApplication::sharedApplication(mtm);
    app.setMainMenu(Some(&menu_bar));
}

/// The key equivalent and modifier mask an NSMenuItem needs to display a
/// binding such as "cmd+option+s". Empty for a binding on a non-character
/// key, which the menu cannot show.
fn menu_key_equivalent(binding: &str) -> (String, objc2_app_kit::NSEventModifierFlags) {
    use crate::keybindings::{parse_key_combo, Key};
    use objc2_app_kit::NSEventModifierFlags;
    let combo = parse_key_combo(binding);
    let Key::Char(c) = combo.key else {
        return (String::new(), NSEventModifierFlags::empty());
    };
    if c == '\0' {
        return (String::new(), NSEventModifierFlags::empty());
    }
    let mut mask = NSEventModifierFlags::empty();
    if combo.cmd { mask |= NSEventModifierFlags::Command; }
    if combo.ctrl { mask |= NSEventModifierFlags::Control; }
    if combo.option { mask |= NSEventModifierFlags::Option; }
    if combo.shift { mask |= NSEventModifierFlags::Shift; }
    (c.to_string(), mask)
}
