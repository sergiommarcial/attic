//! macOS FSEvents as an [`EventSource`].
//!
//! The stream runs on its own thread's run loop and hands each callback to
//! the caller as one [`EventBatch`] over a channel. Events are folder-level
//! (no per-file flag), which is what [`crate::apply::apply_batch`] wants. All
//! FFI for FSEvents lives in this file.

use std::ffi::{CStr, OsStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use core_foundation_sys::array::{CFArrayCreate, CFArrayRef, kCFTypeArrayCallBacks};
use core_foundation_sys::base::{Boolean, CFAllocatorRef, CFIndex, CFRelease, kCFAllocatorDefault};
use core_foundation_sys::date::CFTimeInterval;
use core_foundation_sys::runloop::{
    CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRunInMode, CFRunLoopStop, kCFRunLoopDefaultMode,
};
use core_foundation_sys::string::{CFStringCreateWithBytes, CFStringRef, kCFStringEncodingUTF8};

use crate::events::{Change, EventBatch, EventSource};
use crate::{AtticError, Result};

type StreamRef = *mut c_void;
type Callback = extern "C" fn(StreamRef, *mut c_void, usize, *mut c_void, *const u32, *const u64);

#[repr(C)]
struct Context {
    version: CFIndex,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CFAllocatorRef,
        callback: Callback,
        context: *const Context,
        paths: CFArrayRef,
        since_when: u64,
        latency: CFTimeInterval,
        flags: u32,
    ) -> StreamRef;
    fn FSEventStreamScheduleWithRunLoop(
        stream: StreamRef,
        run_loop: CFRunLoopRef,
        mode: CFStringRef,
    );
    fn FSEventStreamStart(stream: StreamRef) -> Boolean;
    fn FSEventStreamStop(stream: StreamRef);
    fn FSEventStreamInvalidate(stream: StreamRef);
    fn FSEventStreamRelease(stream: StreamRef);
    fn FSEventsGetCurrentEventId() -> u64;
}

const SINCE_NOW: u64 = u64::MAX;
const CREATE_WATCH_ROOT: u32 = 0x04;
const MUST_SCAN_SUB_DIRS: u32 = 0x01;
const USER_DROPPED: u32 = 0x02;
const KERNEL_DROPPED: u32 = 0x04;
const EVENT_IDS_WRAPPED: u32 = 0x08;
const HISTORY_DONE: u32 = 0x10;
const ROOT_CHANGED: u32 = 0x20;
const HISTORY_LOST: u32 = USER_DROPPED | KERNEL_DROPPED | EVENT_IDS_WRAPPED | ROOT_CHANGED;

/// The id of the newest event the system has seen so far. A stream started
/// "since" it delivers only what happens afterwards.
pub fn current_event_id() -> u64 {
    // SAFETY: takes no arguments and only reads a counter.
    unsafe { FSEventsGetCurrentEventId() }
}

/// How often the stream's thread wakes to check whether it should stop.
/// Dropping the source also stops the run loop directly, so this only
/// matters if the drop lands before the loop has started; it is long so an
/// idle service barely wakes.
const STOP_POLL: CFTimeInterval = 10.0;

struct RunLoop(CFRunLoopRef);
// CFRunLoopStop may be called from any thread; that is all this is used for.
unsafe impl Send for RunLoop {}

/// FSEvents for a set of roots.
pub struct FsEventSource {
    batches: Receiver<EventBatch>,
    run_loop: RunLoop,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FsEventSource {
    /// Starts watching `roots`. With `since`, events recorded after that id
    /// (while nothing was watching) are replayed first; without it, only
    /// new events arrive. `latency` is how long FSEvents gathers events
    /// into one batch.
    ///
    /// # Errors
    ///
    /// [`AtticError::Command`] when the stream cannot be created or
    /// started, [`AtticError::Io`] when the thread cannot be spawned.
    pub fn start(
        roots: &[PathBuf],
        since: Option<u64>,
        latency: Duration,
    ) -> Result<FsEventSource> {
        let roots: Vec<Vec<u8>> = roots
            .iter()
            .map(|r| r.as_os_str().as_bytes().to_vec())
            .collect();
        let (batch_tx, batches) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("attic-fsevents".into())
            .spawn(move || run(&roots, since, latency, batch_tx, ready_tx, &stop_flag))
            .map_err(|e| AtticError::io("fsevents thread", e))?;
        let run_loop = ready
            .recv()
            .map_err(|_| failed("the stream's thread exited"))?
            .map_err(failed)?;
        Ok(FsEventSource {
            batches,
            run_loop,
            stop,
            thread: Some(thread),
        })
    }
}

impl EventSource for FsEventSource {
    fn next_batch(&mut self, timeout: Duration) -> Result<Option<EventBatch>> {
        match self.batches.recv_timeout(timeout) {
            Ok(batch) => Ok(Some(batch)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(failed("the stream stopped")),
        }
    }
}

impl Drop for FsEventSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        unsafe { CFRunLoopStop(self.run_loop.0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn failed(message: &str) -> AtticError {
    AtticError::Command {
        program: "fsevents".into(),
        message: message.into(),
    }
}

fn run(
    roots: &[Vec<u8>],
    since: Option<u64>,
    latency: Duration,
    batches: Sender<EventBatch>,
    ready: Sender<std::result::Result<RunLoop, &'static str>>,
    stop: &AtomicBool,
) {
    // SAFETY: every CF object created here is released here; the boxed
    // sender outlives the stream, which is invalidated before it is freed.
    unsafe {
        let strings: Vec<CFStringRef> = roots
            .iter()
            .map(|r| {
                CFStringCreateWithBytes(
                    kCFAllocatorDefault,
                    r.as_ptr(),
                    r.len() as CFIndex,
                    kCFStringEncodingUTF8,
                    0,
                )
            })
            .collect();
        // A path that isn't UTF-8 gives no string; CF must not see a null.
        if strings.iter().any(|s| s.is_null()) {
            for s in strings.into_iter().filter(|s| !s.is_null()) {
                CFRelease(s.cast());
            }
            let _ = ready.send(Err("a root is not valid UTF-8"));
            return;
        }
        let paths = CFArrayCreate(
            kCFAllocatorDefault,
            strings.as_ptr().cast(),
            strings.len() as CFIndex,
            &kCFTypeArrayCallBacks,
        );
        for s in strings {
            CFRelease(s.cast());
        }
        let sender = Box::into_raw(Box::new(batches));
        let context = Context {
            version: 0,
            info: sender.cast(),
            retain: ptr::null(),
            release: ptr::null(),
            copy_description: ptr::null(),
        };
        let stream = FSEventStreamCreate(
            kCFAllocatorDefault,
            callback,
            &context,
            paths,
            since.unwrap_or(SINCE_NOW),
            latency.as_secs_f64(),
            CREATE_WATCH_ROOT,
        );
        CFRelease(paths.cast());
        if stream.is_null() {
            drop(Box::from_raw(sender));
            let _ = ready.send(Err("could not create the stream"));
            return;
        }
        let run_loop = CFRunLoopGetCurrent();
        FSEventStreamScheduleWithRunLoop(stream, run_loop, kCFRunLoopDefaultMode);
        if FSEventStreamStart(stream) == 0 {
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            drop(Box::from_raw(sender));
            let _ = ready.send(Err("could not start the stream"));
            return;
        }
        let _ = ready.send(Ok(RunLoop(run_loop)));
        while !stop.load(Ordering::Relaxed) {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, STOP_POLL, 0);
        }
        FSEventStreamStop(stream);
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
        drop(Box::from_raw(sender));
    }
}

extern "C" fn callback(
    _stream: StreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
) {
    // SAFETY: FSEvents passes `count` C strings, flags and ids, and `info`
    // is the sender boxed in `run`, alive until the stream is released.
    let (sender, paths, flags, ids) = unsafe {
        (
            &*info.cast::<Sender<EventBatch>>(),
            std::slice::from_raw_parts(paths.cast::<*const c_char>(), count),
            std::slice::from_raw_parts(flags, count),
            std::slice::from_raw_parts(ids, count),
        )
    };
    let mut batch = EventBatch {
        changes: Vec::new(),
        last_id: 0,
        history_lost: false,
    };
    for ((&path, &flag), &id) in paths.iter().zip(flags).zip(ids) {
        batch.last_id = batch.last_id.max(id);
        if flag & HISTORY_LOST != 0 {
            batch.history_lost = true;
        }
        if flag & HISTORY_DONE != 0 {
            continue;
        }
        let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        // FSEvents reports folders as "dir/"; the store keys on exact bytes.
        let dir: PathBuf = Path::new(OsStr::from_bytes(bytes)).components().collect();
        batch.changes.push(Change {
            dir,
            subtree: flag & MUST_SCAN_SUB_DIRS != 0,
        });
    }
    if !batch.changes.is_empty() || batch.history_lost {
        let _ = sender.send(batch);
    }
}
