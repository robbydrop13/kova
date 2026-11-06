//! The native half of `voice`: the dictation a chat drives (`Dictation`),
//! its thread (`Session`: the key from the Keychain, the microphone through
//! an AudioQueue at 16 kHz mono, then one request through NSURLSession), the
//! keyboard watch that tells a hold from a tap and catches Escape, and the
//! sounds. Not tested: the pure half (`voice`) is.
//!
//! The key lives in the dictation thread's memory only, from the Keychain to
//! the request's `Authorization` header; nothing writes or logs it. The audio
//! stays in memory and is dropped with the thread.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSSound};
use objc2_foundation::{
    MainThreadMarker, NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURLResponse, NSURLSession,
    NSURLSessionConfiguration, NSURL,
};

use crate::voice::{self, Command as Cmd, Failure, Flash, Heard, History, Keys, Pill};

static HISTORY: Mutex<History> = Mutex::new(History::new());

/// What a chat's keyboard watch saw, with when: drained by its poll.
type KeyInbox = Arc<Mutex<Vec<KeyInput>>>;

#[derive(Clone, Copy, Debug)]
enum KeyInput {
    Release(u64),
    Escape,
}

fn history() -> Vec<String> {
    HISTORY.lock().map(|h| h.items()).unwrap_or_default()
}

/// One chat's voice typing: the shortcut or the mic button in, the pill and
/// the text to insert out.
#[derive(Default)]
pub struct Dictation {
    keys: Keys,
    session: Option<Session>,
    watch: Option<KeyWatch>,
    keys_in: KeyInbox,
    flash: Option<Flash>,
}

/// What a poll hands the chat.
pub struct Polled {
    pub pill: Pill,
    /// Clean text to insert at the cursor.
    pub text: Option<String>,
}

impl Dictation {
    /// The shortcut went down. From the keyboard, its key is watched for the
    /// release (hold or tap); from IPC there is no key: a tap.
    pub fn shortcut(&mut self) {
        let now = crate::surface::monotonic_ms();
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let key = NSApplication::sharedApplication(mtm)
            .currentEvent()
            .filter(|e| e.r#type() == NSEventType::KeyDown)
            .map(|e| (e.keyCode(), e.isARepeat()));
        match key {
            Some((_, true)) => {}
            Some((code, false)) => self.press(now, Some(code)),
            None => {
                self.press(now, None);
                self.release(now);
            }
        }
    }

    /// A press: the shortcut (with its key code) or the mic button.
    pub fn press(&mut self, now: u64, key: Option<u16>) {
        let cmd = self.keys.press(now);
        self.run(cmd, now);
        if cmd == Cmd::Start {
            self.watch = KeyWatch::install(key, self.keys_in.clone());
        }
    }

    pub fn release(&mut self, now: u64) {
        let cmd = self.keys.release(now);
        self.run(cmd, now);
    }

    pub fn finish(&mut self) {
        let cmd = self.keys.finish();
        self.run(cmd, crate::surface::monotonic_ms());
    }

    pub fn cancel(&mut self) {
        let cmd = self.keys.cancel();
        self.run(cmd, crate::surface::monotonic_ms());
    }

    fn run(&mut self, cmd: Cmd, now: u64) {
        match cmd {
            Cmd::Start => {
                self.flash = None;
                if let Ok(mut keys) = self.keys_in.lock() {
                    keys.clear();
                }
                play("Tink");
                self.session = Some(Session::start());
            }
            Cmd::Finish => {
                play("Pop");
                if let Some(session) = &self.session {
                    session.finish();
                }
            }
            Cmd::Cancel => {
                if let Some(session) = self.session.take() {
                    session.cancel();
                }
                self.watch = None;
                self.flash = Some(Flash::cancelled(now));
            }
            Cmd::Nothing => {}
        }
    }

    pub fn poll(&mut self) -> Polled {
        let now = crate::surface::monotonic_ms();
        let inputs = self.keys_in.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default();
        for input in inputs {
            match input {
                KeyInput::Release(at) => self.release(at),
                KeyInput::Escape => self.cancel(),
            }
        }
        let mut text = None;
        let notes = self.session.as_ref().map(Session::take).unwrap_or_default();
        for note in notes {
            match note {
                Note::Full => self.finish(),
                Note::Heard(heard) => {
                    if heard.cleanup_failed {
                        log::info!("voice: cleanup failed, the verbatim text goes in");
                    }
                    if let Ok(mut h) = HISTORY.lock() {
                        h.push(&heard.text);
                    }
                    text = Some(heard.text);
                }
                Note::Failed(f) => {
                    if !f.quiet() {
                        play("Basso");
                    }
                    self.flash = Some(Flash::failure(f, now));
                }
                Note::Closed => {
                    self.session = None;
                    self.keys.done();
                }
            }
        }
        if !self.keys.active() {
            self.watch = None;
        }
        let level = self.session.as_ref().map(Session::level).unwrap_or(0.0);
        Polled { pill: voice::pill(self.keys.phase, self.flash.as_ref(), now, level, history()), text }
    }
}

fn play(name: &str) {
    if let Some(sound) = NSSound::soundNamed(&NSString::from_str(name)) {
        sound.play();
    }
}

/// A local event monitor while a dictation runs: the release of the
/// shortcut's key (or of Cmd: a Cmd combo's key-up may never reach the
/// app), and Escape, which goes no further.
struct KeyWatch {
    monitor: Retained<AnyObject>,
}

impl KeyWatch {
    fn install(key: Option<u16>, inbox: KeyInbox) -> Option<KeyWatch> {
        let released = std::cell::Cell::new(key.is_none());
        let handler = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
            let e = unsafe { event.as_ref() };
            let push = |input| {
                if let Ok(mut keys) = inbox.lock() {
                    keys.push(input);
                }
            };
            match e.r#type() {
                NSEventType::KeyDown if e.keyCode() == 53 => {
                    push(KeyInput::Escape);
                    return std::ptr::null_mut();
                }
                NSEventType::KeyUp if !released.get() && Some(e.keyCode()) == key => {
                    released.set(true);
                    push(KeyInput::Release(crate::surface::monotonic_ms()));
                }
                NSEventType::FlagsChanged if !released.get() && !e.modifierFlags().contains(NSEventModifierFlags::Command) => {
                    released.set(true);
                    push(KeyInput::Release(crate::surface::monotonic_ms()));
                }
                _ => {}
            }
            event.as_ptr()
        });
        let mask = NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged;
        let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) }?;
        Some(KeyWatch { monitor })
    }
}

impl Drop for KeyWatch {
    fn drop(&mut self) {
        unsafe { NSEvent::removeMonitor(&self.monitor) };
    }
}

/// What the dictation thread reports.
#[derive(Debug)]
enum Note {
    /// 20 min reached: listening stopped, the audio is on its way.
    Full,
    Heard(Heard),
    Failed(Failure),
    Closed,
}

type Inbox = Arc<Mutex<Vec<Note>>>;

fn push(inbox: &Inbox, note: Note) {
    if let Ok(mut list) = inbox.lock() {
        list.push(note);
    }
}

const RUN: u8 = 0;
const FINISH: u8 = 1;
const CANCEL: u8 = 2;

/// One dictation's thread: listening until `finish` (then the request) or
/// `cancel` (then nothing).
struct Session {
    control: Arc<AtomicU8>,
    level: Arc<AtomicU32>,
    inbox: Inbox,
}

impl Session {
    fn start() -> Session {
        let control = Arc::new(AtomicU8::new(RUN));
        let level = Arc::new(AtomicU32::new(0));
        let inbox: Inbox = Arc::new(Mutex::new(Vec::new()));
        let (c, l, i) = (control.clone(), level.clone(), inbox.clone());
        let spawned = std::thread::Builder::new().name("kova-voice".into()).spawn(move || {
            run(&c, &l, &i);
            push(&i, Note::Closed);
        });
        if spawned.is_err() {
            push(&inbox, Note::Failed(Failure::Failed));
            push(&inbox, Note::Closed);
        }
        Session { control, level, inbox }
    }

    fn finish(&self) {
        let _ = self.control.compare_exchange(RUN, FINISH, Ordering::Relaxed, Ordering::Relaxed);
    }

    fn cancel(&self) {
        self.control.store(CANCEL, Ordering::Relaxed);
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    fn take(&self) -> Vec<Note> {
        self.inbox.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn keychain_key() -> Option<String> {
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", voice::KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run(control: &AtomicU8, level: &AtomicU32, inbox: &Inbox) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    // The microphone first: the first words are not lost to the Keychain read.
    let mic = match Mic::start(tx) {
        Ok(mic) => mic,
        Err(status) => {
            log::warn!("voice: no microphone (AudioQueue status {status})");
            return push(inbox, Note::Failed(Failure::NoMic));
        }
    };
    let key = match voice::api_key(keychain_key) {
        Ok(key) => key,
        Err(_) => return push(inbox, Note::Failed(Failure::NoKey)),
    };
    log::info!("voice: listening");
    let max = voice::MAX_AUDIO_MS * voice::BYTES_PER_MS;
    let mut audio: Vec<u8> = Vec::with_capacity(voice::SAMPLE_RATE as usize * 2 * 10);
    let started = std::time::Instant::now();
    while control.load(Ordering::Relaxed) == RUN {
        // By the clock too: a microphone that delivers nothing still stops.
        if started.elapsed() >= Duration::from_millis(voice::MAX_AUDIO_MS as u64) {
            push(inbox, Note::Full);
            break;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                level.store(voice::level(&chunk).to_bits(), Ordering::Relaxed);
                audio.extend_from_slice(&chunk);
                if audio.len() >= max {
                    audio.truncate(max);
                    push(inbox, Note::Full);
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(mic);
    level.store(0, Ordering::Relaxed);
    if control.load(Ordering::Relaxed) == CANCEL {
        return log::info!("voice: cancelled");
    }
    while let Ok(chunk) = rx.try_recv() {
        audio.extend_from_slice(&chunk);
    }
    audio.truncate(max);
    if audio.len() < voice::MIN_AUDIO_MS * voice::BYTES_PER_MS {
        return log::info!("voice: too short, nothing sent");
    }
    let outcome = dictate(&key, audio, control);
    drop(key);
    if control.load(Ordering::Relaxed) == CANCEL {
        return log::info!("voice: cancelled");
    }
    match outcome {
        Ok(heard) if heard.text.is_empty() => push(inbox, Note::Failed(Failure::NothingHeard)),
        Ok(heard) => push(inbox, Note::Heard(heard)),
        Err(f) => push(inbox, Note::Failed(f)),
    }
}

/// The request, retried like Claap's back does (3 attempts on 429 and
/// gateway errors, or no connection), until the dictation is cancelled.
fn dictate(key: &str, audio: Vec<u8>, control: &AtomicU8) -> Result<Heard, Failure> {
    let boundary = format!("kova-{:x}", crate::surface::monotonic_ms() ^ (std::process::id() as u64) << 32);
    let rules = std::env::var_os("HOME").and_then(|h| std::fs::read_to_string(std::path::Path::new(&h).join(voice::INSTRUCTION_FILE)).ok());
    let body = voice::multipart(&boundary, &voice::config_json(&voice::instruction(rules), &voice::vocabulary()), &audio);
    // 20 min is 38 MB: the body holds it, the audio goes.
    drop(audio);
    let mut last = Failure::Unreachable;
    for attempt in 0..3u32 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(400 << attempt));
            if control.load(Ordering::Relaxed) == CANCEL {
                return Err(Failure::Failed);
            }
        }
        match post(key, &boundary, &body, control) {
            Some((200, answer)) => return voice::parse_result(&String::from_utf8_lossy(&answer)).ok_or(Failure::Failed),
            Some((status, _)) => {
                // Never the body: an error page may echo the request.
                log::warn!("voice: AssemblyAI answered {status}");
                last = Failure::from_status(status);
                if !voice::retriable(status) {
                    return Err(last);
                }
            }
            None => {
                log::warn!("voice: AssemblyAI unreachable");
                last = Failure::Unreachable;
            }
        }
    }
    Err(last)
}

/// One POST through an ephemeral NSURLSession (no cache, no cookies):
/// the status and body, or `None` without an answer. Esc or the wait running
/// out cancels the upload: up to 38 MB, it is not left running.
fn post(key: &str, boundary: &str, body: &[u8], control: &AtomicU8) -> Option<(u16, Vec<u8>)> {
    let url = NSURL::URLWithString(&NSString::from_str(voice::DICTATION_URL))?;
    let request = NSMutableURLRequest::requestWithURL(&url);
    request.setHTTPMethod(&NSString::from_str("POST"));
    request.setValue_forHTTPHeaderField(Some(&NSString::from_str(key)), &NSString::from_str("Authorization"));
    let kind = format!("multipart/form-data; boundary={boundary}");
    request.setValue_forHTTPHeaderField(Some(&NSString::from_str(&kind)), &NSString::from_str("Content-Type"));
    request.setHTTPBody(Some(&NSData::with_bytes(body)));
    let wait = voice::request_timeout_secs(body.len());
    request.setTimeoutInterval(wait as f64 - 5.0);
    let session = NSURLSession::sessionWithConfiguration(&NSURLSessionConfiguration::ephemeralSessionConfiguration());
    let (tx, rx) = mpsc::channel::<Option<(u16, Vec<u8>)>>();
    let done = block2::RcBlock::new(move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
        let answer = if !error.is_null() || response.is_null() {
            None
        } else {
            let status = unsafe { &*response }.downcast_ref::<NSHTTPURLResponse>().map(|r| r.statusCode() as u16);
            let bytes = if data.is_null() { Vec::new() } else { unsafe { &*data }.to_vec() };
            status.map(|s| (s, bytes))
        };
        let _ = tx.send(answer);
    });
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &done) };
    task.resume();
    let deadline = std::time::Instant::now() + Duration::from_secs(wait);
    let answer = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => break answer,
            Err(RecvTimeoutError::Timeout) if control.load(Ordering::Relaxed) != CANCEL && std::time::Instant::now() < deadline => {}
            Err(_) => break None,
        }
    };
    session.invalidateAndCancel();
    answer
}

// ---------------------------------------------------------------
// The microphone: an AudioQueue input at 16 kHz mono, 16-bit
// ---------------------------------------------------------------

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioQueueBuffer {
    capacity: u32,
    data: *mut std::ffi::c_void,
    size: u32,
    user_data: *mut std::ffi::c_void,
    packet_description_capacity: u32,
    packet_descriptions: *mut std::ffi::c_void,
    packet_description_count: u32,
}

type AudioQueueRef = *mut std::ffi::c_void;
type InputCallback = extern "C" fn(*mut std::ffi::c_void, AudioQueueRef, *mut AudioQueueBuffer, *const std::ffi::c_void, u32, *const std::ffi::c_void);

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioQueueNewInput(
        format: *const AudioStreamBasicDescription,
        callback: InputCallback,
        user_data: *mut std::ffi::c_void,
        run_loop: *const std::ffi::c_void,
        run_loop_mode: *const std::ffi::c_void,
        flags: u32,
        out: *mut AudioQueueRef,
    ) -> i32;
    fn AudioQueueAllocateBuffer(queue: AudioQueueRef, size: u32, out: *mut *mut AudioQueueBuffer) -> i32;
    fn AudioQueueEnqueueBuffer(queue: AudioQueueRef, buffer: *mut AudioQueueBuffer, n: u32, descs: *const std::ffi::c_void) -> i32;
    fn AudioQueueStart(queue: AudioQueueRef, start: *const std::ffi::c_void) -> i32;
    fn AudioQueueStop(queue: AudioQueueRef, immediate: u8) -> i32;
    fn AudioQueueDispose(queue: AudioQueueRef, immediate: u8) -> i32;
}

const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
/// Signed integer, packed.
const PCM_FLAGS: u32 = 4 | 8;
/// 100 ms of audio per buffer, three in flight.
const BUFFER_BYTES: u32 = voice::SAMPLE_RATE / 10 * 2;
const BUFFERS: usize = 3;

struct Mic {
    queue: AudioQueueRef,
    sender: *mut Sender<Vec<u8>>,
}

extern "C" fn on_input(user: *mut std::ffi::c_void, queue: AudioQueueRef, buffer: *mut AudioQueueBuffer, _: *const std::ffi::c_void, _: u32, _: *const std::ffi::c_void) {
    let (sender, buf) = unsafe { (&*(user as *const Sender<Vec<u8>>), &*buffer) };
    if buf.size > 0 && !buf.data.is_null() {
        let bytes = unsafe { std::slice::from_raw_parts(buf.data as *const u8, buf.size as usize) };
        let _ = sender.send(bytes.to_vec());
    }
    unsafe { AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null()) };
}

impl Mic {
    /// The first start asks macOS for the microphone (`NSMicrophoneUsageDescription`).
    fn start(tx: Sender<Vec<u8>>) -> Result<Mic, i32> {
        let format = AudioStreamBasicDescription {
            sample_rate: voice::SAMPLE_RATE as f64,
            format_id: LINEAR_PCM,
            format_flags: PCM_FLAGS,
            bytes_per_packet: 2,
            frames_per_packet: 1,
            bytes_per_frame: 2,
            channels_per_frame: 1,
            bits_per_channel: 16,
            reserved: 0,
        };
        let sender = Box::into_raw(Box::new(tx));
        let mut queue: AudioQueueRef = std::ptr::null_mut();
        let status = unsafe {
            AudioQueueNewInput(&format, on_input, sender.cast(), std::ptr::null(), std::ptr::null(), 0, &mut queue)
        };
        let mic = Mic { queue, sender };
        if status != 0 {
            return Err(status);
        }
        for _ in 0..BUFFERS {
            let mut buffer = std::ptr::null_mut();
            let status = unsafe { AudioQueueAllocateBuffer(queue, BUFFER_BYTES, &mut buffer) };
            if status != 0 {
                return Err(status);
            }
            unsafe { AudioQueueEnqueueBuffer(queue, buffer, 0, std::ptr::null()) };
        }
        let status = unsafe { AudioQueueStart(queue, std::ptr::null()) };
        if status != 0 {
            return Err(status);
        }
        Ok(mic)
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        if !self.queue.is_null() {
            // Synchronous: no callback runs after this, so the sender can go.
            unsafe {
                AudioQueueStop(self.queue, 1);
                AudioQueueDispose(self.queue, 1);
            }
        }
        // The sender is left behind (a few bytes per dictation): Apple does
        // not promise a callback still running has returned by now.
        let _ = self.sender;
    }
}
