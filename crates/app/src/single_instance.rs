//! Single-instance guard built on a named mutex.
//!
//! Two instances would both install low-level hooks, so a solo Alt press would
//! be suppressed twice and the IME key injected twice.
//!
//! The name has no `Global\` prefix, which puts it in the session-local
//! namespace: one instance per logged-on session, so a second user signing in
//! can still run their own.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::wide;

pub struct SingleInstance {
    handle: HANDLE,
}

/// Take the lock for `name`. Returns `None` if another instance already holds it.
pub fn acquire(name: &str) -> Option<SingleInstance> {
    let wide_name = wide(name);
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide_name.as_ptr()) };
    if handle.is_null() {
        return None;
    }
    // The handle is returned even when the mutex already existed, so the
    // existing-instance case has to be read from the error code.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return None;
    }
    Some(SingleInstance { handle })
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}
