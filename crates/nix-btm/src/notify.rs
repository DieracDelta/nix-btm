//! Platform-specific notification system for ring buffer readers.
//!
//! - Linux: Uses `io_uring` with FutexWake/FutexWait
//! - macOS: Uses kqueue with `EVFILT_USER`

#[cfg(target_os = "linux")]
mod platform {
    use io_uring::{IoUring, Probe, opcode::FutexWake};
    use libc::FUTEX_BITSET_MATCH_ANY;
    use snafu::GenerateImplicitData;

    use crate::{protocol_common::ProtocolError, ring_writer::MAX_NUM_CLIENTS};

    const FUTEX_MAGIC_NUMBER: u64 = 0xf00f00;

    pub struct Notifier {
        uring: IoUring,
    }

    impl Notifier {
        pub fn new() -> Result<Option<Self>, ProtocolError> {
            if std::env::var("DISABLE_IO_URING").is_ok() {
                eprintln!(
                    "Info: io_uring disabled via DISABLE_IO_URING, using \
                     POSIX mode (polling only)"
                );
                return Ok(None);
            }

            match IoUring::new(MAX_NUM_CLIENTS) {
                Ok(uring) => {
                    let mut probe = Probe::new();
                    let _ = uring.submitter().register_probe(&mut probe);
                    if probe.is_supported(FutexWake::CODE) {
                        Ok(Some(Self { uring }))
                    } else {
                        eprintln!(
                            "Warning: io_uring FutexWake not supported, \
                             falling back to POSIX mode (polling only)"
                        );
                        Ok(None)
                    }
                }
                Err(_) => {
                    eprintln!(
                        "Warning: io_uring not available, falling back to \
                         POSIX mode (polling only)"
                    );
                    Ok(None)
                }
            }
        }

        pub fn wake(&mut self, addr: *const u32) -> Result<(), ProtocolError> {
            let nr_wake: u64 = u64::MAX;
            let flags: u32 = FUTEX_BITSET_MATCH_ANY as u32;
            let mask: u64 = 0;

            let sqe = FutexWake::new(addr, nr_wake, mask, flags)
                .build()
                .user_data(FUTEX_MAGIC_NUMBER);

            unsafe {
                self.uring.submission().push(&sqe).ok();
            }
            self.uring.submit().map_err(|source| ProtocolError::Io {
                source,
                backtrace: snafu::Backtrace::generate(),
            })?;

            Ok(())
        }
    }

    pub struct Waiter {
        uring: IoUring,
    }

    impl Waiter {
        pub fn new() -> Result<Option<Self>, ProtocolError> {
            use io_uring::opcode::FutexWait;

            if std::env::var("DISABLE_IO_URING").is_ok() {
                eprintln!(
                    "Info: io_uring disabled via DISABLE_IO_URING, using \
                     POSIX mode (polling only)"
                );
                return Ok(None);
            }

            match IoUring::new(MAX_NUM_CLIENTS) {
                Ok(uring) => {
                    let mut probe = Probe::new();
                    let _ = uring.submitter().register_probe(&mut probe);
                    if probe.is_supported(FutexWait::CODE) {
                        Ok(Some(Self { uring }))
                    } else {
                        eprintln!(
                            "Warning: io_uring FutexWait not supported, \
                             falling back to POSIX mode (polling only)"
                        );
                        Ok(None)
                    }
                }
                Err(_) => {
                    eprintln!(
                        "Warning: io_uring not available, falling back to \
                         POSIX mode (polling only)"
                    );
                    Ok(None)
                }
            }
        }

        /// Wait for the value at `addr` to change from `expected`.
        /// Returns immediately if the value is already different.
        pub fn wait(
            &mut self,
            addr: *const u32,
            expected: u32,
        ) -> Result<(), ProtocolError> {
            use io_uring::opcode::FutexWait;

            let flags: u32 = FUTEX_BITSET_MATCH_ANY as u32;
            let mask: u64 = 0;

            let sqe = FutexWait::new(addr, expected as u64, mask, flags)
                .build()
                .user_data(FUTEX_MAGIC_NUMBER);

            unsafe {
                self.uring.submission().push(&sqe).ok();
            }
            // TODO this does NOT integrate well with tokio. IMO it's a better
            // choice to switch to a io uring first async runtime.
            self.uring.submit_and_wait(1).map_err(|source| {
                ProtocolError::Io {
                    source,
                    backtrace: snafu::Backtrace::generate(),
                }
            })?;

            // Consume the completion
            self.uring.completion().next();

            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    use snafu::GenerateImplicitData;

    use crate::protocol_common::ProtocolError;

    /// Notifier using kqueue's `EVFILT_USER` for macOS
    pub struct Notifier {
        kq: OwnedFd,
    }

    impl Notifier {
        pub fn new() -> Result<Option<Self>, ProtocolError> {
            if std::env::var("DISABLE_KQUEUE").is_ok() {
                eprintln!(
                    "Info: kqueue disabled via DISABLE_KQUEUE, using POSIX \
                     mode (polling only)"
                );
                return Ok(None);
            }

            let kq = unsafe { libc::kqueue() };
            if kq < 0 {
                eprintln!(
                    "Warning: kqueue not available, falling back to POSIX \
                     mode (polling only)"
                );
                return Ok(None);
            }

            let kq = unsafe { OwnedFd::from_raw_fd(kq) };

            // Register the EVFILT_USER event before we can trigger it
            let event = libc::kevent {
                ident: 1, // Fixed identifier for ring buffer notifications
                filter: libc::EVFILT_USER,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };

            let ret = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    &raw const event,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };

            if ret < 0 {
                eprintln!(
                    "Warning: Failed to register kqueue event, falling back \
                     to POSIX mode"
                );
                return Ok(None);
            }

            Ok(Some(Self { kq }))
        }

        pub fn wake(&mut self, _addr: *const u32) -> Result<(), ProtocolError> {
            // For kqueue, we use EVFILT_USER to trigger a user event
            // The addr parameter is ignored on macOS - we use a fixed ident
            let event = libc::kevent {
                ident: 1, // Fixed identifier for ring buffer notifications
                filter: libc::EVFILT_USER,
                flags: 0,
                fflags: libc::NOTE_TRIGGER,
                data: 0,
                udata: std::ptr::null_mut(),
            };

            let ret = unsafe {
                libc::kevent(
                    self.kq.as_raw_fd(),
                    &raw const event,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };

            if ret < 0 {
                return Err(ProtocolError::Io {
                    source: std::io::Error::last_os_error(),
                    backtrace: snafu::Backtrace::generate(),
                });
            }

            Ok(())
        }
    }

    /// Waiter using kqueue for macOS
    pub struct Waiter {
        kq: OwnedFd,
    }

    impl Waiter {
        pub fn new() -> Result<Option<Self>, ProtocolError> {
            if std::env::var("DISABLE_KQUEUE").is_ok() {
                eprintln!(
                    "Info: kqueue disabled via DISABLE_KQUEUE, using POSIX \
                     mode (polling only)"
                );
                return Ok(None);
            }

            let kq = unsafe { libc::kqueue() };
            if kq < 0 {
                eprintln!(
                    "Warning: kqueue not available, falling back to POSIX \
                     mode (polling only)"
                );
                return Ok(None);
            }

            let kq = unsafe { OwnedFd::from_raw_fd(kq) };

            // Register for EVFILT_USER events
            let event = libc::kevent {
                ident: 1,
                filter: libc::EVFILT_USER,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };

            let ret = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    &raw const event,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };

            if ret < 0 {
                eprintln!(
                    "Warning: Failed to register kqueue event, falling back \
                     to POSIX mode"
                );
                return Ok(None);
            }

            Ok(Some(Self { kq }))
        }

        /// Wait for a notification from the daemon.
        /// The `addr` and `expected` parameters are ignored on macOS since
        /// we use `EVFILT_USER` instead of futex.
        pub fn wait(
            &mut self,
            _addr: *const u32,
            _expected: u32,
        ) -> Result<(), ProtocolError> {
            let mut event_out = libc::kevent {
                ident: 0,
                filter: 0,
                flags: 0,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };

            let ret = unsafe {
                libc::kevent(
                    self.kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &raw mut event_out,
                    1,
                    std::ptr::null(), // No timeout - block indefinitely
                )
            };

            if ret < 0 {
                return Err(ProtocolError::Io {
                    source: std::io::Error::last_os_error(),
                    backtrace: snafu::Backtrace::generate(),
                });
            }

            Ok(())
        }
    }
}

pub use platform::{Notifier, Waiter};
