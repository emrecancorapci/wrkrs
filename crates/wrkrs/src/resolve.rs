//! The system resolver backing wrk.lookup and wrk.connect.
//!
//! Lookup goes through getaddrinfo with AF_UNSPEC and SOCK_STREAM, the
//! same hints the C tool passes, and probes use blocking connects. The
//! resolver remembers the text of the last failed probe so the host
//! can report it where wrk reads errno.

use std::ffi::{CStr, CString};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};
use std::ptr;
use std::sync::Mutex;

use wrkrs_engine::ResolveApi;

/// Resolves through the system resolver and probes with TCP connects.
pub struct SystemResolver {
    last_connect_error: Mutex<Option<String>>,
}

impl SystemResolver {
    /// Creates a resolver with no probe failure recorded.
    pub fn new() -> SystemResolver {
        SystemResolver {
            last_connect_error: Mutex::new(None),
        }
    }

    /// The strerror shaped text of the last failed probe.
    ///
    /// Reads errno where wrk does, so a run with no recorded failure
    /// answers the strerror of zero.
    pub fn last_connect_error(&self) -> String {
        self.last_connect_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| "Success".to_owned())
    }
}

impl Default for SystemResolver {
    fn default() -> Self {
        SystemResolver::new()
    }
}

impl ResolveApi for SystemResolver {
    fn lookup(&self, host: &str, service: &str) -> io::Result<Vec<SocketAddr>> {
        lookup(host, service)
    }

    fn connect(&self, address: &SocketAddr) -> bool {
        match TcpStream::connect(address) {
            Ok(_) => true,
            Err(error) => {
                let mut last = self
                    .last_connect_error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *last = Some(strip_os_suffix(&error.to_string()));
                false
            }
        }
    }
}

/// Calls getaddrinfo with the wrk hints and keeps the result order.
fn lookup(host: &str, service: &str) -> io::Result<Vec<SocketAddr>> {
    let host = CString::new(host).map_err(|_| io::Error::other("invalid host"))?;
    let service = CString::new(service).map_err(|_| io::Error::other("invalid service"))?;
    let hints = libc::addrinfo {
        ai_flags: 0,
        ai_family: libc::AF_UNSPEC,
        ai_socktype: libc::SOCK_STREAM,
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: ptr::null_mut(),
        ai_canonname: ptr::null_mut(),
        ai_next: ptr::null_mut(),
    };
    let mut result: *mut libc::addrinfo = ptr::null_mut();

    // SAFETY: the hints struct and the result pointer are valid for
    // the call and both strings are valid C strings. The result list
    // is walked and freed before returning.
    let code = unsafe { libc::getaddrinfo(host.as_ptr(), service.as_ptr(), &hints, &mut result) };
    if code != 0 {
        // SAFETY: gai_strerror answers a static string for the code.
        let text = unsafe { libc::gai_strerror(code) };
        let message = if text.is_null() {
            "name resolution failed".to_owned()
        } else {
            // SAFETY: the pointer came from gai_strerror above.
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .to_string()
        };
        return Err(io::Error::other(message));
    }

    let mut addresses = Vec::new();
    let mut current = result;
    while !current.is_null() {
        // SAFETY: current walks the list returned by getaddrinfo.
        let info = unsafe { &*current };
        if !info.ai_addr.is_null() {
            // SAFETY: ai_addr points at a sockaddr of ai_addrlen bytes
            // matching the reported family.
            let address = unsafe { socket_address(info) };
            if let Some(address) = address {
                addresses.push(address);
            }
        }
        current = info.ai_next;
    }

    // SAFETY: the list came from getaddrinfo above.
    unsafe { libc::freeaddrinfo(result) };
    Ok(addresses)
}

/// Converts one addrinfo entry into a socket address.
///
/// # Safety
///
/// `info.ai_addr` must point at a valid sockaddr for the family.
unsafe fn socket_address(info: &libc::addrinfo) -> Option<SocketAddr> {
    if info.ai_family == libc::AF_INET {
        let sockaddr = unsafe { &*(info.ai_addr as *const libc::sockaddr_in) };
        let address = Ipv4Addr::from(sockaddr.sin_addr.s_addr.to_ne_bytes());
        return Some(SocketAddr::V4(SocketAddrV4::new(
            address,
            u16::from_be(sockaddr.sin_port),
        )));
    }
    if info.ai_family == libc::AF_INET6 {
        let sockaddr = unsafe { &*(info.ai_addr as *const libc::sockaddr_in6) };
        let address = Ipv6Addr::from(sockaddr.sin6_addr.s6_addr);
        return Some(SocketAddr::V6(SocketAddrV6::new(
            address,
            u16::from_be(sockaddr.sin6_port),
            sockaddr.sin6_flowinfo,
            sockaddr.sin6_scope_id,
        )));
    }
    let _ = IpAddr::V4(Ipv4Addr::LOCALHOST);
    None
}

/// Drops the os error suffix so the text matches strerror.
fn strip_os_suffix(text: &str) -> String {
    match text.find(" (os error") {
        Some(index) => text[..index].to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::{SystemResolver, strip_os_suffix};
    use wrkrs_engine::ResolveApi;

    #[test]
    fn strips_the_os_suffix() {
        assert_eq!(
            strip_os_suffix("Connection refused (os error 111)"),
            "Connection refused"
        );
    }

    #[test]
    fn resolves_loopback_in_resolver_order() {
        let resolver = SystemResolver::new();
        let addresses = resolver
            .lookup("127.0.0.1", "8080")
            .expect("numeric lookup works");
        assert_eq!(addresses.len(), 1);
    }

    #[test]
    fn lookup_failures_carry_the_gai_text() {
        let resolver = SystemResolver::new();
        let error = resolver
            .lookup("wrkrs.invalid", "http")
            .expect_err("the invalid TLD never resolves");
        let message = error.to_string();
        assert!(
            !message.contains("failed to lookup address information"),
            "message must carry the gai text only: {message}"
        );
    }

    #[test]
    fn refused_probes_record_the_strerror_text() {
        let resolver = SystemResolver::new();
        let address = "127.0.0.1:1".parse().unwrap();
        assert!(!resolver.connect(&address));
        assert_eq!(resolver.last_connect_error(), "Connection refused");
    }

    #[test]
    fn accepted_probes_succeed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local address");
        let resolver = SystemResolver::new();
        assert!(resolver.connect(&address));
        assert_eq!(resolver.last_connect_error(), "Success");
    }
}
