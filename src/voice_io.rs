//! The native half of `voice`: the key from the Keychain, a temporary token
//! minted with it, the microphone through an AudioQueue (16 kHz mono PCM,
//! Core Audio converts), the socket through NSURLSession. Not tested: the
//! pure half (`voice`) is.
//!
//! One thread per dictation. The key lives in that thread's memory only: it
//! goes to `curl` on stdin (never on a command line other processes can
//! read), and the socket only ever sees the token.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{
    NSData, NSError, NSString, NSURLSession, NSURLSessionWebSocketCloseCode, NSURLSessionWebSocketMessage,
    NSURLSessionWebSocketMessageType, NSURLSessionWebSocketTask, NSURL,
};

use crate::voice;

/// What the dictation thread reports to the chat.
#[derive(Debug)]
pub enum Note {
    Event(voice::Event),
    /// A line for the user: no key, no microphone, the server refused.
    Failed(String),
    /// The thread is done.
    Closed,
}

type Inbox = Arc<Mutex<Vec<Note>>>;

fn push(inbox: &Inbox, note: Note) {
    if let Ok(mut list) = inbox.lock() {
        list.push(note);
    }
}

/// One dictation: listening from `start` until `stop` (or a failure).
pub struct Session {
    stop: Arc<AtomicBool>,
    inbox: Inbox,
}

impl Session {
    pub fn start() -> Session {
        let stop = Arc::new(AtomicBool::new(false));
        let inbox: Inbox = Arc::new(Mutex::new(Vec::new()));
        let (s, i) = (stop.clone(), inbox.clone());
        let spawned = std::thread::Builder::new().name("kova-voice".into()).spawn(move || {
            run(&s, &i);
            push(&i, Note::Closed);
        });
        if spawned.is_err() {
            push(&inbox, Note::Failed("Dictée indisponible".into()));
            push(&inbox, Note::Closed);
        }
        Session { stop, inbox }
    }

    /// Stop listening; what was said still arrives, then `Closed`.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn take(&self) -> Vec<Note> {
        self.inbox.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
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

/// A temporary streaming token, the key passed to curl as a header file on stdin.
fn mint_token(key: &str) -> Option<String> {
    let mut child = Command::new("/usr/bin/curl")
        .args(["-sS", "-m", "10", "-X", "POST", voice::TOKEN_URL, "-H", "@-", "-H", "Content-Type: application/json", "-d", voice::TOKEN_BODY])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        let mut stdin = child.stdin.take()?;
        stdin.write_all(format!("Authorization: {key}\n").as_bytes()).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    voice::token_from(&String::from_utf8_lossy(&out.stdout))
}

fn run(stop: &AtomicBool, inbox: &Inbox) {
    let key = match voice::api_key(keychain_key) {
        Ok(key) => key,
        Err(line) => return push(inbox, Note::Failed(line.into())),
    };
    let token = mint_token(&key);
    drop(key);
    let Some(token) = token else {
        return push(inbox, Note::Failed("AssemblyAI a refusé la clé".into()));
    };
    let closed = Arc::new(AtomicBool::new(false));
    let Some(socket) = Socket::open(&voice::ws_url(&token), inbox.clone(), closed.clone()) else {
        return push(inbox, Note::Failed("Connexion AssemblyAI impossible".into()));
    };
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mic = match Mic::start(tx) {
        Ok(mic) => mic,
        Err(status) => {
            log::warn!("voice: no microphone (AudioQueue status {status})");
            socket.close();
            return push(inbox, Note::Failed("Micro indisponible".into()));
        }
    };
    log::info!("voice: listening");
    while !stop.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => socket.send_binary(&chunk),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(mic);
    // The open turn is finalised on Terminate: wait for it, a moment at most.
    if !closed.load(Ordering::Relaxed) {
        socket.send_text(r#"{"type":"Terminate"}"#);
        let until = Instant::now() + Duration::from_secs(3);
        while !closed.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    socket.close();
    log::info!("voice: stopped");
}

/// The socket: messages read in a loop on NSURLSession's queue, each pushed
/// to the inbox as a `voice::Event`; `closed` once the server ends it.
struct Socket {
    task: Retained<NSURLSessionWebSocketTask>,
    closed: Arc<AtomicBool>,
}

impl Socket {
    fn open(url: &str, inbox: Inbox, closed: Arc<AtomicBool>) -> Option<Socket> {
        let url = NSURL::URLWithString(&NSString::from_str(url))?;
        let task = NSURLSession::sharedSession().webSocketTaskWithURL(&url);
        task.resume();
        receive(task.clone(), inbox, closed.clone());
        Some(Socket { task, closed })
    }

    fn send(&self, message: &NSURLSessionWebSocketMessage) {
        let closed = self.closed.clone();
        let done = block2::RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() {
                closed.store(true, Ordering::Relaxed);
            }
        });
        unsafe { self.task.sendMessage_completionHandler(message, &done) };
    }

    fn send_binary(&self, bytes: &[u8]) {
        let data = NSData::with_bytes(bytes);
        self.send(&NSURLSessionWebSocketMessage::initWithData(NSURLSessionWebSocketMessage::alloc(), &data));
    }

    fn send_text(&self, text: &str) {
        self.send(&NSURLSessionWebSocketMessage::initWithString(NSURLSessionWebSocketMessage::alloc(), &NSString::from_str(text)));
    }

    fn close(&self) {
        self.task.cancelWithCloseCode_reason(NSURLSessionWebSocketCloseCode::NormalClosure, None);
    }
}

/// Read one message, then arm the next read: NSURLSession reads one at a time.
fn receive(task: Retained<NSURLSessionWebSocketTask>, inbox: Inbox, closed: Arc<AtomicBool>) {
    let next = task.clone();
    let handler = block2::RcBlock::new(move |message: *mut NSURLSessionWebSocketMessage, error: *mut NSError| {
        if !error.is_null() || message.is_null() {
            if !closed.swap(true, Ordering::Relaxed) && !error.is_null() {
                let reason = unsafe { &*error }.localizedDescription().to_string();
                log::info!("voice: socket closed ({reason})");
            }
            return;
        }
        let message = unsafe { &*message };
        if message.r#type() == NSURLSessionWebSocketMessageType::String
            && let Some(text) = message.string()
        {
            let event = voice::parse(&text.to_string());
            let end = event == voice::Event::Termination;
            if let voice::Event::Error(e) = &event {
                log::warn!("voice: server error: {e}");
                push(&inbox, Note::Failed("Erreur AssemblyAI".into()));
            }
            push(&inbox, Note::Event(event));
            if end {
                closed.store(true, Ordering::Relaxed);
                return;
            }
        }
        receive(next.clone(), inbox.clone(), closed.clone());
    });
    unsafe { task.receiveMessageWithCompletionHandler(&handler) };
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
        drop(unsafe { Box::from_raw(self.sender) });
    }
}
