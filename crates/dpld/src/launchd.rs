//! launchd socket activation — how dpl gets :80/:443 without ever running as
//! root, and without pf.
//!
//! A LaunchDaemon (installed once by `dpl setup`) declares the two sockets.
//! launchd, which *is* root, binds them at boot and then spawns this daemon as
//! the logged-in user, handing over the already-bound descriptors. We ask for
//! them by the name used in the plist's `Sockets` dict.
//!
//! The alternative — a pf `rdr` anchor from :80 to :8080 — put the redirect in
//! global, mutable system state that any `pfctl -f /etc/pf.conf` (a VPN client,
//! Docker, a reboot) silently flushed, leaving every site dead on the clean URL.
//! Nothing can flush a file descriptor.

/// Listeners handed to us by launchd under `name`, or `None` when this process
/// wasn't launched by launchd (a bare `dpld` from a shell), or the job has no
/// socket by that name — both of which are normal, and mean "bind it yourself".
#[cfg(target_os = "macos")]
pub fn activated(name: &str) -> Option<Vec<std::net::TcpListener>> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::io::FromRawFd;

    // Both live in libSystem, which every macOS binary links already.
    unsafe extern "C" {
        fn launch_activate_socket(name: *const c_char, fds: *mut *mut c_int, count: *mut usize) -> c_int;
        fn free(ptr: *mut c_void);
    }

    let cname = CString::new(name).ok()?;
    let mut fds: *mut c_int = std::ptr::null_mut();
    let mut count: usize = 0;

    // Non-zero is the errno: ESRCH when we aren't a launchd job, ENOENT when the
    // job exists but declares no socket under this name.
    let rc = unsafe { launch_activate_socket(cname.as_ptr(), &mut fds, &mut count) };
    if rc != 0 || fds.is_null() || count == 0 {
        if rc != 0 && rc != 3 {
            tracing::debug!(name, errno = rc, "launch_activate_socket declined");
        }
        return None;
    }

    // The array is malloc'd by launchd; the descriptors in it are ours to own.
    let slice = unsafe { std::slice::from_raw_parts(fds, count) };
    let listeners: Vec<std::net::TcpListener> =
        slice.iter().map(|&fd| unsafe { std::net::TcpListener::from_raw_fd(fd) }).collect();
    unsafe { free(fds.cast::<c_void>()) };

    Some(listeners)
}

#[cfg(not(target_os = "macos"))]
pub fn activated(_name: &str) -> Option<Vec<std::net::TcpListener>> {
    None
}

#[cfg(test)]
mod tests {
    /// The test binary is not a launchd job, so activation must decline rather
    /// than hand back a bogus descriptor. This also proves the symbol links.
    #[test]
    fn declines_when_not_a_launchd_job() {
        assert!(super::activated("http").is_none());
    }

    #[test]
    fn declines_for_an_unknown_socket_name() {
        assert!(super::activated("no-such-socket").is_none());
    }
}

/// The single activated listener for `name`, ready for tokio.
///
/// Our plist pins each socket to one `SockNodeName`, so launchd hands back
/// exactly one descriptor. If a hand-edited plist widens that, we serve the
/// first and close the rest rather than leave them bound but never accepted —
/// a listening socket nobody accepts on hangs clients instead of refusing them.
pub fn activated_listener(name: &str) -> Option<(tokio::net::TcpListener, u16)> {
    let mut listeners = activated(name)?;
    if listeners.len() > 1 {
        tracing::warn!(name, count = listeners.len(), "launchd passed several sockets; using the first");
    }
    let std_listener = listeners.drain(..).next()?;
    std_listener.set_nonblocking(true).ok()?;
    let port = std_listener.local_addr().ok()?.port();
    match tokio::net::TcpListener::from_std(std_listener) {
        Ok(l) => Some((l, port)),
        Err(e) => {
            tracing::warn!(name, error = %e, "adopting the launchd socket failed");
            None
        }
    }
}
