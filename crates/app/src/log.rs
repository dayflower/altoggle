//! Logging that is safe to call from a low-level hook callback.
//!
//! The callback must return within `LowLevelHooksTimeout` (300ms by default) or
//! Windows silently drops the hook, so it must never block on I/O. Writing to a
//! console can block for an unbounded time: selecting text with QuickEdit pauses
//! every write to that console.
//!
//! So the callback only pushes a `String` onto a channel, and a dedicated thread
//! does the formatting and the writing.

use std::io::{BufWriter, Write};
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;

enum Cmd {
    Line(String),
    Stop,
}

static TX: OnceLock<Sender<Cmd>> = OnceLock::new();
static WRITER: OnceLock<JoinHandle<()>> = OnceLock::new();

/// Start the writer thread. Calling it more than once is a no-op.
pub fn init() {
    if TX.get().is_some() {
        return;
    }
    let (tx, rx) = channel::<Cmd>();
    let handle = std::thread::spawn(move || {
        // Never hold stdout.lock() across the loop: that would deadlock any
        // other thread trying to print. BufWriter<Stdout> locks per write.
        let mut out = BufWriter::new(std::io::stdout());
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Line(s) => {
                    let _ = writeln!(out, "{s}");
                    let _ = out.flush();
                }
                Cmd::Stop => break,
            }
        }
    });
    let _ = TX.set(tx);
    let _ = WRITER.set(handle);
}

/// Queue one line. Safe from a hook callback: it never blocks on I/O.
pub fn line(s: impl Into<String>) {
    if let Some(tx) = TX.get() {
        // A failed send means the writer is gone. Swallow it; panicking inside a
        // hook callback would take the whole desktop's input down with it.
        let _ = tx.send(Cmd::Line(s.into()));
    }
}

/// Drain the queue and stop the writer thread.
///
/// The sender lives in a static for the life of the process, so dropping it
/// cannot close the channel; the writer needs an explicit stop message.
pub fn shutdown() {
    if let Some(tx) = TX.get() {
        let _ = tx.send(Cmd::Stop);
    }
}
