//! The event notification backend, named the way aeGetApiName reports
//! it for the version line.

#[cfg(target_os = "linux")]
pub const NAME: &str = "epoll";

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub const NAME: &str = "kqueue";

#[cfg(target_os = "solaris")]
pub const NAME: &str = "evport";

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_os = "solaris"
)))]
pub const NAME: &str = "select";
