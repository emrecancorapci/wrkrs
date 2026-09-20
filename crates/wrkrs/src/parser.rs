//! URL parsing over the vendored http_parser grammar.
//!
//! Ports http_parser_parse_url (strict mode, the way wrk builds it)
//! together with the reads script.c and wrk.c make on the result. The
//! byte ranges the state machine marks are validated against the C
//! parser in the golden tests.

use std::ops::Range;

use wrkrs_engine::UrlRef;

/// The fields the URL parser marks, as byte ranges into the URL.
#[derive(Default)]
struct Fields {
    schema: Option<Range<usize>>,
    host: Option<Range<usize>>,
    port: Option<Range<usize>>,
    path: Option<Range<usize>>,
}

/// Parses a benchmark URL into the parts wrk exposes.
///
/// Returns `None` for every URL the C parser rejects. wrk additionally
/// requires a schema and a host.
pub fn parse_url(url: &str) -> Option<UrlRef> {
    let fields = parse_fields(url)?;
    let schema = fields.schema.as_ref()?;
    let host = fields.host.as_ref()?;
    let port = fields
        .port
        .as_ref()
        .map(|range| url[range.clone()].to_owned());
    // script.c slices from the path offset to the end of the URL, so
    // the query and any fragment ride along.
    let path = match &fields.path {
        Some(range) => url[range.start..].to_owned(),
        None => "/".to_owned(),
    };
    Some(UrlRef {
        scheme: Some(url[schema.clone()].to_owned()),
        host: Some(url[host.clone()].to_owned()),
        port,
        path,
    })
}

/// The URL state machine, ported from parse_url_char.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UrlState {
    Dead,
    SpacesBeforeUrl,
    Schema,
    SchemaSlash,
    SchemaSlashSlash,
    ServerWithAt,
    ServerStart,
    Server,
    Path,
    QueryStart,
    Query,
    FragmentStart,
    Fragment,
}

/// The host state machine, ported from http_parse_host_char.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HostState {
    Dead,
    UserInfoStart,
    UserInfo,
    HostStart,
    Host,
    V6Start,
    V6,
    V6End,
    V6ZoneStart,
    V6Zone,
    PortStart,
    Port,
}

/// Runs the main URL state machine and the host split.
fn parse_fields(url: &str) -> Option<Fields> {
    let bytes = url.as_bytes();
    let mut fields = Fields::default();
    let mut state = UrlState::SpacesBeforeUrl;
    // The main loop marks the whole authority as the host field, the
    // host split narrows it afterwards.
    let mut authority: Option<Range<usize>> = None;
    let mut found_at = false;

    let mut index = 0;
    while index < bytes.len() {
        state = url_step(state, bytes[index]);
        match state {
            UrlState::Dead => return None,
            UrlState::SchemaSlash
            | UrlState::SchemaSlashSlash
            | UrlState::ServerStart
            | UrlState::QueryStart
            | UrlState::FragmentStart => {}
            UrlState::ServerWithAt => {
                found_at = true;
                extend(index, &mut authority);
            }
            UrlState::Server => extend(index, &mut authority),
            UrlState::Schema => extend(index, &mut fields.schema),
            UrlState::Path => extend(index, &mut fields.path),
            UrlState::Query | UrlState::Fragment => {}
            UrlState::SpacesBeforeUrl => {}
        }
        index += 1;
    }

    // Host must be present when there is a schema, and wrk requires
    // both.
    let authority = authority?;
    fields.schema.as_ref()?;
    let (host, port) = split_host(url, authority, found_at)?;
    fields.host = Some(host);
    fields.port = port;

    if let Some(range) = &fields.port {
        let digits = &url[range.clone()];
        let value: u64 = digits.parse().ok()?;
        if value > 0xffff {
            return None;
        }
    }

    Some(fields)
}

/// Grows a field range by one byte.
fn extend(index: usize, range: &mut Option<Range<usize>>) {
    match range {
        Some(existing) => existing.end = index + 1,
        None => *range = Some(index..index + 1),
    }
}

/// One step of the URL state machine.
fn url_step(state: UrlState, ch: u8) -> UrlState {
    if ch == b' ' || ch == b'\r' || ch == b'\n' || ch == b'\t' || ch == b'\x0c' {
        return UrlState::Dead;
    }
    match state {
        UrlState::SpacesBeforeUrl => {
            if ch == b'/' || ch == b'*' {
                UrlState::Path
            } else if ch.is_ascii_alphabetic() {
                UrlState::Schema
            } else {
                UrlState::Dead
            }
        }
        UrlState::Schema => {
            if ch.is_ascii_alphabetic() {
                UrlState::Schema
            } else if ch == b':' {
                UrlState::SchemaSlash
            } else {
                UrlState::Dead
            }
        }
        UrlState::SchemaSlash => {
            if ch == b'/' {
                UrlState::SchemaSlashSlash
            } else {
                UrlState::Dead
            }
        }
        UrlState::SchemaSlashSlash => {
            if ch == b'/' {
                UrlState::ServerStart
            } else {
                UrlState::Dead
            }
        }
        UrlState::ServerWithAt => {
            if ch == b'@' {
                UrlState::Dead
            } else {
                url_step(UrlState::Server, ch)
            }
        }
        UrlState::ServerStart | UrlState::Server => {
            if ch == b'/' {
                UrlState::Path
            } else if ch == b'?' {
                UrlState::QueryStart
            } else if ch == b'@' {
                UrlState::ServerWithAt
            } else if is_userinfo_char(ch) || ch == b'[' || ch == b']' {
                UrlState::Server
            } else {
                UrlState::Dead
            }
        }
        UrlState::Path => {
            if is_url_char(ch) {
                UrlState::Path
            } else if ch == b'?' {
                UrlState::QueryStart
            } else if ch == b'#' {
                UrlState::FragmentStart
            } else {
                UrlState::Dead
            }
        }
        UrlState::QueryStart | UrlState::Query => {
            if is_url_char(ch) || ch == b'?' {
                UrlState::Query
            } else if ch == b'#' {
                UrlState::FragmentStart
            } else {
                UrlState::Dead
            }
        }
        UrlState::FragmentStart => {
            if is_url_char(ch) || ch == b'?' {
                UrlState::Fragment
            } else if ch == b'#' {
                UrlState::FragmentStart
            } else {
                UrlState::Dead
            }
        }
        UrlState::Fragment => {
            if is_url_char(ch) || ch == b'?' || ch == b'#' {
                UrlState::Fragment
            } else {
                UrlState::Dead
            }
        }
        UrlState::Dead => UrlState::Dead,
    }
}

/// Splits the authority into host and port, ported from
/// http_parse_host.
fn split_host(
    url: &str,
    authority: Range<usize>,
    found_at: bool,
) -> Option<(Range<usize>, Option<Range<usize>>)> {
    let bytes = url.as_bytes();
    let mut host = authority.clone();
    host.end = authority.start;
    let mut port: Option<Range<usize>> = None;

    let mut state = if found_at {
        HostState::UserInfoStart
    } else {
        HostState::HostStart
    };

    for index in authority {
        let next = host_step(state, bytes[index]);
        if next == HostState::Dead {
            return None;
        }
        match next {
            HostState::Host => {
                if state != HostState::Host {
                    host.start = index;
                    host.end = index;
                }
                host.end = index + 1;
            }
            HostState::V6 => {
                if state != HostState::V6 {
                    host.start = index;
                    host.end = index;
                }
                host.end = index + 1;
            }
            HostState::V6ZoneStart | HostState::V6Zone => {
                host.end = index + 1;
            }
            HostState::Port => match &mut port {
                Some(existing) => existing.end = index + 1,
                None => port = Some(index..index + 1),
            },
            _ => {}
        }
        state = next;
    }

    match state {
        HostState::HostStart
        | HostState::V6Start
        | HostState::V6
        | HostState::V6ZoneStart
        | HostState::V6Zone
        | HostState::PortStart
        | HostState::UserInfo
        | HostState::UserInfoStart => None,
        _ => Some((host, port)),
    }
}

/// One step of the host state machine.
fn host_step(state: HostState, ch: u8) -> HostState {
    match state {
        HostState::UserInfo | HostState::UserInfoStart => {
            if ch == b'@' {
                HostState::HostStart
            } else if is_userinfo_char(ch) {
                HostState::UserInfo
            } else {
                HostState::Dead
            }
        }
        HostState::HostStart => {
            if ch == b'[' {
                HostState::V6Start
            } else if is_host_char(ch) {
                HostState::Host
            } else {
                HostState::Dead
            }
        }
        HostState::Host => {
            if is_host_char(ch) {
                HostState::Host
            } else if ch == b':' {
                HostState::PortStart
            } else {
                HostState::Dead
            }
        }
        HostState::V6End => {
            if ch == b':' {
                HostState::PortStart
            } else {
                HostState::Dead
            }
        }
        HostState::V6 => {
            if ch == b']' {
                HostState::V6End
            } else if ch.is_ascii_hexdigit() || ch == b':' || ch == b'.' {
                HostState::V6
            } else if ch == b'%' {
                HostState::V6ZoneStart
            } else {
                HostState::Dead
            }
        }
        HostState::V6Start => {
            if ch.is_ascii_hexdigit() || ch == b':' || ch == b'.' {
                HostState::V6
            } else {
                HostState::Dead
            }
        }
        HostState::V6Zone => {
            if ch == b']' {
                HostState::V6End
            } else if is_zone_char(ch) {
                HostState::V6Zone
            } else {
                HostState::Dead
            }
        }
        HostState::V6ZoneStart => {
            if is_zone_char(ch) {
                HostState::V6Zone
            } else {
                HostState::Dead
            }
        }
        HostState::Port | HostState::PortStart => {
            if ch.is_ascii_digit() {
                HostState::Port
            } else {
                HostState::Dead
            }
        }
        HostState::Dead => HostState::Dead,
    }
}

/// IS_USERINFO_CHAR from http_parser.c.
fn is_userinfo_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            b'-' | b'_'
                | b'.'
                | b'!'
                | b'~'
                | b'*'
                | b'\''
                | b'('
                | b')'
                | b'%'
                | b';'
                | b':'
                | b'&'
                | b'='
                | b'+'
                | b'$'
                | b','
        )
}

/// IS_HOST_CHAR in strict mode.
fn is_host_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'.' || ch == b'-'
}

/// Zone ID characters for IPv6 hosts, RFC 6874.
fn is_zone_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, b'%' | b'.' | b'-' | b'_' | b'~')
}

/// IS_URL_CHAR, the strict normal_url_char bitmap.
fn is_url_char(ch: u8) -> bool {
    matches!(ch,
        b'\t' | b'\x0c'
        | b'!' | b'"'
        | 0x24..=0x3e
        | b'A'..=b'Z'
        | b'^' | b'_'
        | b'`'
        | b'a'..=b'z'
        | b'|'
        | b'~')
}

#[cfg(test)]
mod tests {
    use super::parse_url;
    use wrkrs_engine::UrlRef;

    fn url_ref(scheme: &str, host: &str, port: Option<&str>, path: &str) -> UrlRef {
        UrlRef {
            scheme: Some(scheme.to_owned()),
            host: Some(host.to_owned()),
            port: port.map(str::to_owned),
            path: path.to_owned(),
        }
    }

    #[test]
    fn parses_plain_urls() {
        assert_eq!(
            parse_url("http://127.0.0.1:8080/some/path"),
            Some(url_ref("http", "127.0.0.1", Some("8080"), "/some/path"))
        );
        assert_eq!(
            parse_url("http://example.test/"),
            Some(url_ref("http", "example.test", None, "/"))
        );
    }

    #[test]
    fn keeps_the_query_and_fragment_in_the_path() {
        // script.c slices from the path offset to the end of the URL.
        assert_eq!(
            parse_url("http://127.0.0.1:8080/some/path?b=c&d=e"),
            Some(url_ref(
                "http",
                "127.0.0.1",
                Some("8080"),
                "/some/path?b=c&d=e"
            ))
        );
        assert_eq!(
            parse_url("http://host/p#frag"),
            Some(url_ref("http", "host", None, "/p#frag"))
        );
    }

    #[test]
    fn a_query_without_a_path_falls_back_to_the_slash() {
        assert_eq!(
            parse_url("http://host?q=1"),
            Some(url_ref("http", "host", None, "/"))
        );
    }

    #[test]
    fn ipv6_hosts_lose_their_brackets() {
        assert_eq!(
            parse_url("http://[::1]:8080/"),
            Some(url_ref("http", "::1", Some("8080"), "/"))
        );
        assert_eq!(
            parse_url("http://[fe80::1%25eth0]:1/"),
            Some(url_ref("http", "fe80::1%25eth0", Some("1"), "/"))
        );
    }

    #[test]
    fn userinfo_is_not_part_of_the_host() {
        assert_eq!(
            parse_url("http://u:p@host:80/x"),
            Some(url_ref("http", "host", Some("80"), "/x"))
        );
    }

    #[test]
    fn accepts_any_schema_case() {
        assert_eq!(
            parse_url("HTTP://host/"),
            Some(url_ref("HTTP", "host", None, "/"))
        );
        assert_eq!(parse_url("h://x/"), Some(url_ref("h", "x", None, "/")));
    }

    #[test]
    fn rejects_invalid_urls() {
        let invalid = [
            "http:///nopath",
            "http://host:",
            "http://host:80x/",
            "http://host:99999/",
            "http://ho st/",
            "/relative",
            " http://h/",
            "1http://h/",
            "http://h/a b",
            "http://",
            "http://:80/",
            "http://[::1",
            "",
        ];
        for url in invalid {
            assert_eq!(parse_url(url), None, "{url} must be invalid");
        }
    }
}
