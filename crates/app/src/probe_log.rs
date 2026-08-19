//! stdout logging that is safe to call from a probe's hook callback.
//!
//! Same reasoning as [`crate::log`], different destination: the callback must
//! return within `LowLevelHooksTimeout` (300ms by default) or Windows drops the
//! hook without telling anyone, and a console write can block for an unbounded
//! time — selecting text with QuickEdit pauses every write to that console. So
//! the callback only queues, and a dedicated thread does the writing.
//!
//! The probes print to stdout rather than `OutputDebugStringW` because reading
//! their output as it happens is the whole point of running one.

use std::io::{BufWriter, Write};
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

enum Cmd {
    Line(String),
    /// A line whose text cannot be produced inside the callback.
    ///
    /// `imeprobe` reads the IME back after a fire, and that goes through IMM32,
    /// which waits on another process's message loop. Queueing the closure moves
    /// the whole of it — the wait for the switch to land included — onto the
    /// writer thread, which has no deadline.
    Deferred(Box<dyn FnOnce() -> String + Send>),
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
        .name("probe-log".into())
        .spawn(move || {
            // Holding stdout.lock() would deadlock the main thread's println!.
            // BufWriter<Stdout> takes the lock per write, so it is safe.
            let mut out = BufWriter::new(std::io::stdout());
            while let Ok(cmd) = rx.recv() {
                let line = match cmd {
                    Cmd::Line(s) => s,
                    Cmd::Deferred(f) => f(),
                    Cmd::Stop => break,
                };
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
            }
        });
    match handle {
        Ok(handle) => {
            let _ = TX.set(tx);
            *writer_slot() = Some(handle);
        }
        // Without a writer everything is dropped silently. Losing the log is not
        // a reason to refuse to run.
        Err(_) => eprintln!("could not start the log writer thread"),
    }
}

/// Queue one line. Safe from a hook callback: it never blocks on I/O.
pub fn line(s: impl Into<String>) {
    send(Cmd::Line(s.into()));
}

/// Queue one line to be composed on the writer thread.
///
/// For anything the callback must not do itself. See [`Cmd::Deferred`].
pub fn deferred(f: impl FnOnce() -> String + Send + 'static) {
    send(Cmd::Deferred(Box::new(f)));
}

fn send(cmd: Cmd) {
    if let Some(tx) = TX.get() {
        // A failed send means the writer is gone. Swallow it; panicking inside a
        // hook callback would take the whole desktop's input down with it.
        let _ = tx.send(cmd);
    }
}

/// Drain the queue and wait for the writer to finish.
///
/// Call this before exiting, or the last lines never make it out. The sender
/// lives in a static for the life of the process, so dropping it cannot close
/// the channel; the writer needs an explicit stop.
pub fn shutdown() {
    send(Cmd::Stop);
    if let Some(handle) = writer_slot().take() {
        let _ = handle.join();
    }
}
