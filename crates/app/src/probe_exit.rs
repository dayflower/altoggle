//! The two ways a probe stops on its own.
//!
//! A probe blocks real trigger ups, so a wedged message loop would take the
//! keyboard with it. Both probes therefore quit on Ctrl+C and again after a
//! deadline, and both do it by posting `WM_QUIT` rather than by terminating:
//! the normal path out of the loop is what unhooks and releases the modifiers.
//!
//! The app does not use this. It has no console in a release build, so its
//! escape hatches are the tray's Quit and `--exit-after`, both of which go
//! through `session::request_quit`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};

/// The thread running the message loop, as recorded by [`arm`].
static LOOP_TID: AtomicU32 = AtomicU32::new(0);

fn request_quit() {
    let tid = LOOP_TID.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
    }
}

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> i32 {
    // Letting the default termination run could leave a blocked trigger stuck
    // down, and a stuck Win key turns every later keystroke into a hotkey.
    request_quit();
    1
}

/// Arm both hatches on the calling thread, which must be the one that pumps the
/// message loop. `secs` of 0 leaves Ctrl+C as the only one.
///
/// Call this before installing any hook: from that moment on there has to be a
/// way out.
pub fn arm(secs: u64) {
    LOOP_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), 1) };
    if secs > 0 {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            request_quit();
        });
    }
}
