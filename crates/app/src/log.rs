//! Logging that is safe to call from a low-level hook callback.
//!
//! The callback must return within `LowLevelHooksTimeout` (300ms by default) or
//! Windows silently drops the hook, so it must never block on I/O. Writing to a
//! console can block for an unbounded time: selecting text with QuickEdit pauses
//! every write to that console.
//!
//! So the callback only pushes a `String` onto a channel, and a dedicated thread
//! does the writing.
//!
//! Output goes to `OutputDebugStringW`, which works whether or not the process
//! owns a console; a release build has none. Attach DebugView (or any debugger)
//! to read it. Debug builds also echo to stdout, since `cargo run` has a console
//! right there.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;

use crate::wide;

/// Distinguishes our lines from every other process's output in DebugView.
const PREFIX: &str = "altoggle: ";

enum Cmd {
    Line(String),
    Stop,
}

static TX: OnceLock<Sender<Cmd>> = OnceLock::new();
static WRITER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

fn writer_slot() -> MutexGuard<'static, Option<JoinHandle<()>>> {
    WRITER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start the writer thread. Calling it more than once is a no-op.
pub fn init() {
    if TX.get().is_some() {
        return;
    }
    let (tx, rx) = channel::<Cmd>();
    let handle = std::thread::Builder::new()
        .name("altoggle-log".into())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Cmd::Line(s) => {
                        let text = format!("{PREFIX}{s}\n");
                        // Safe here and only here: this thread has no deadline.
                        unsafe { OutputDebugStringW(wide(&text).as_ptr()) };
                        if cfg!(debug_assertions) {
                            print!("{text}");
                        }
                    }
                    Cmd::Stop => break,
                }
            }
        });
    match handle {
        Ok(handle) => {
            let _ = TX.set(tx);
            *writer_slot() = Some(handle);
        }
        // Without a writer, `line` silently drops everything. Losing the log is
        // not a reason to refuse to run.
        Err(_) => {
            let text = format!("{PREFIX}could not start the log writer thread\n");
            unsafe { OutputDebugStringW(wide(&text).as_ptr()) };
        }
    }
}

/// Queue one line. Safe from a hook callback: it never blocks on I/O.
pub fn line(s: impl Into<String>) {
    if let Some(tx) = TX.get() {
        // A failed send means the writer is gone. Swallow it; panicking inside a
        // hook callback would take the whole desktop's input down with it.
        let _ = tx.send(Cmd::Line(s.into()));
    }
}

/// Drain the queue and wait for the writer to finish.
///
/// Call this before exiting, including on error paths, or the last lines never
/// make it out. The sender lives in a static for the life of the process, so
/// dropping it cannot close the channel; the writer needs an explicit stop.
pub fn shutdown() {
    if let Some(tx) = TX.get() {
        let _ = tx.send(Cmd::Stop);
    }
    if let Some(handle) = writer_slot().take() {
        let _ = handle.join();
    }
}
