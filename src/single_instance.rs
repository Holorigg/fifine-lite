//! One running copy per user session. A second launch signals the first one to show
//! its window and then exits.
#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub struct Instance {
        mutex: HANDLE,
        show_event: HANDLE,
    }

    // The handles are only used from the UI thread, but Windows handles are thread-agnostic.
    unsafe impl Send for Instance {}

    impl Instance {
        /// Returns `None` when another copy is already running (after asking it to show itself).
        pub fn acquire() -> Option<Self> {
            unsafe {
                let mutex = CreateMutexW(std::ptr::null(), 0, wide("Local\\fifine-am8-lite-instance").as_ptr());
                let already = GetLastError() == ERROR_ALREADY_EXISTS;
                // Auto-reset event the running copy polls.
                let show_event = CreateEventW(std::ptr::null(), 0, 0, wide("Local\\fifine-am8-lite-show").as_ptr());
                if already {
                    if !show_event.is_null() {
                        SetEvent(show_event);
                        CloseHandle(show_event);
                    }
                    if !mutex.is_null() {
                        CloseHandle(mutex);
                    }
                    return None;
                }
                Some(Self { mutex, show_event })
            }
        }

        /// True once per request from a second launch.
        pub fn show_requested(&self) -> bool {
            !self.show_event.is_null() && unsafe { WaitForSingleObject(self.show_event, 0) } == WAIT_OBJECT_0
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.show_event);
                CloseHandle(self.mutex);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct Instance;
    impl Instance {
        pub fn acquire() -> Option<Self> {
            Some(Self)
        }
        pub fn show_requested(&self) -> bool {
            false
        }
    }
}

pub use imp::Instance;
