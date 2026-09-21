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

/// Verifies a generated request pipeline and counts its messages.
///
/// Frames requests through the headers and any Content-Length body,
/// mirroring script_verify_request. The error carries the parser
/// message with the wrk `line:column` suffix over the consumed bytes.
/// The position points at the start of a malformed message where the
/// C parser points at the offending byte, and chunked request bodies
/// frame as empty, both documented divergences of the httparse glue.
pub fn verify_request(request: &[u8]) -> Result<u64, String> {
    let mut count = 0;
    let mut offset = 0;
    while offset < request.len() {
        match frame(&request[offset..]) {
            Framing::Complete { headers, body } => {
                let next = offset + headers + body;
                if next > request.len() {
                    return Err(diagnostic("incomplete request", request, request.len()));
                }
                offset = next;
                count += 1;
            }
            Framing::Incomplete => {
                return Err(diagnostic("incomplete request", request, request.len()));
            }
            Framing::Malformed(message) => {
                return Err(diagnostic(&message, request, offset));
            }
        }
    }
    if count == 0 {
        return Err(diagnostic("incomplete request", request, request.len()));
    }
    Ok(count)
}

/// The framing result for one message.
enum Framing {
    /// Headers end and body length in bytes.
    Complete { headers: usize, body: usize },
    /// The input ends inside the message.
    Incomplete,
    /// The input is not a request, carrying the parser message.
    Malformed(String),
}

/// Frames one request out of the input.
fn frame(input: &[u8]) -> Framing {
    // Grow the header capacity on demand, scripts can send plenty.
    for capacity in [16usize, 64, 256] {
        let mut headers = vec![httparse::EMPTY_HEADER; capacity];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(input) {
            Ok(httparse::Status::Complete(header_end)) => {
                let length = request
                    .headers
                    .iter()
                    .find(|header| header.name.eq_ignore_ascii_case("content-length"))
                    .and_then(|header| std::str::from_utf8(header.value).ok())
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                return Framing::Complete {
                    headers: header_end,
                    body: length,
                };
            }
            Ok(httparse::Status::Partial) => return Framing::Incomplete,
            Err(httparse::Error::TooManyHeaders) => continue,
            Err(error) => return Framing::Malformed(error.to_string()),
        }
    }
    Framing::Malformed("too many headers".to_owned())
}

/// Renders the wrk diagnostic with the line and column.
///
/// The column counts one plus the bytes since the last newline over
/// the consumed prefix, the same walk as the C loop.
fn diagnostic(message: &str, request: &[u8], consumed: usize) -> String {
    let mut line = 1;
    let mut column = 1;
    let consumed = consumed.min(request.len());
    for byte in &request[..consumed] {
        column += 1;
        if *byte == b'\n' {
            column = 1;
            line += 1;
        }
    }
    format!("{message} at {line}:{column}")
}

/// One completed response.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// Every header pair in arrival order, duplicates included.
    pub headers: Vec<(String, String)>,
    /// The body bytes.
    pub body: Vec<u8>,
    /// Whether the message keeps the connection alive.
    pub keep_alive: bool,
}

/// A streaming HTTP response framer over a byte feed.
///
/// Mirrors the joyent parser behavior wrk depends on: responses frame
/// through Content-Length or chunked, responses without framing read
/// until the connection closes, 1xx, 204, and 304 carry no body, and
/// keep-alive follows the version and the Connection header. Bodies of
/// HEAD responses misframe the same way the C parser does without
/// method context.
pub struct ResponseFramer {
    buf: Vec<u8>,
    scanned: usize,
    state: FrameState,
    capture: bool,
    status: u16,
    message_keep_alive: bool,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Where the framer stands inside the current message.
enum FrameState {
    Headers,
    Body {
        remaining: usize,
    },
    Chunked(ChunkState),
    UntilClose,
    /// A fatal framing problem, the connection reconnects.
    Broken,
}

/// The chunked transfer sub-state.
enum ChunkState {
    Size,
    Data { remaining: usize },
    Trailers,
}

impl ResponseFramer {
    /// Creates a framer. Header and body bytes are captured only when
    /// the script wants responses, the way wrk buffers on demand.
    pub fn new(capture: bool) -> ResponseFramer {
        ResponseFramer {
            buf: Vec::new(),
            scanned: 0,
            state: FrameState::Headers,
            capture,
            status: 0,
            message_keep_alive: true,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Feeds bytes and returns every completed response.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Response>, FrameError> {
        self.compact();
        self.buf.extend_from_slice(data);
        self.drain()
    }

    /// Closes the stream, completing an until-close response.
    pub fn closed(&mut self) -> Result<Vec<Response>, FrameError> {
        match self.state {
            FrameState::Headers | FrameState::Body { .. } | FrameState::Chunked(_) => {
                Err(FrameError::ClosedMidMessage)
            }
            FrameState::UntilClose => {
                if self.capture {
                    self.body.extend_from_slice(&self.buf[self.scanned..]);
                }
                self.scanned = self.buf.len();
                Ok(vec![self.finish()])
            }
            FrameState::Broken => Err(FrameError::Broken),
        }
    }

    /// Consumes the buffer while messages complete.
    fn drain(&mut self) -> Result<Vec<Response>, FrameError> {
        let mut completed = Vec::new();
        loop {
            match &self.state {
                FrameState::Headers => {
                    if let Some(message) = self.frame_headers()? {
                        completed.push(message);
                    } else if matches!(self.state, FrameState::Headers) {
                        // The headers are still partial.
                        break;
                    }
                }
                FrameState::Body { .. } => {
                    if let Some(message) = self.frame_body() {
                        completed.push(message);
                    } else {
                        break;
                    }
                }
                FrameState::Chunked(_) => {
                    if let Some(message) = self.frame_chunked()? {
                        completed.push(message);
                    } else {
                        break;
                    }
                }
                FrameState::UntilClose | FrameState::Broken => break,
            }
        }
        Ok(completed)
    }

    /// Parses the status line and headers, choosing the body framing.
    fn frame_headers(&mut self) -> Result<Option<Response>, FrameError> {
        for capacity in [16usize, 64] {
            let mut headers = vec![httparse::EMPTY_HEADER; capacity];
            let mut response = httparse::Response::new(&mut headers);
            match response.parse(&self.buf[self.scanned..]) {
                Ok(httparse::Status::Complete(header_end)) => {
                    self.status = response.code.unwrap_or(0);
                    // Keep-alive defaults open on 1.1 and closed on 1.0,
                    // the Connection header overrides both ways.
                    self.message_keep_alive = response.version == Some(1);
                    self.headers.clear();
                    for header in response.headers.iter() {
                        let value = String::from_utf8_lossy(header.value).to_string();
                        if header.name.eq_ignore_ascii_case("connection") {
                            let policy = value.to_ascii_lowercase();
                            if policy.contains("close") {
                                self.message_keep_alive = false;
                            }
                            if policy.contains("keep-alive") {
                                self.message_keep_alive = true;
                            }
                        }
                        if self.capture {
                            self.headers.push((header.name.to_owned(), value));
                        }
                    }
                    self.scanned += header_end;

                    // Bodies follow framing headers, some statuses
                    // never carry one.
                    let bodiless = self.status < 200 || self.status == 204 || self.status == 304;
                    let length = response
                        .headers
                        .iter()
                        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
                        .and_then(|header| std::str::from_utf8(header.value).ok())
                        .and_then(|value| value.parse::<usize>().ok());
                    let chunked = response.headers.iter().any(|header| {
                        header.name.eq_ignore_ascii_case("transfer-encoding")
                            && std::str::from_utf8(header.value)
                                .is_ok_and(|value| value.to_ascii_lowercase().contains("chunked"))
                    });

                    if bodiless {
                        // A close on a bodiless message still holds.
                        return Ok(Some(self.finish()));
                    }
                    if let Some(length) = length {
                        self.state = FrameState::Body { remaining: length };
                    } else if chunked {
                        self.state = FrameState::Chunked(ChunkState::Size);
                    } else {
                        // No framing: the body runs until the peer
                        // closes, so the message cannot keep alive.
                        self.message_keep_alive = false;
                        self.state = FrameState::UntilClose;
                    }
                    return Ok(None);
                }
                Ok(httparse::Status::Partial) => return Ok(None),
                Err(httparse::Error::TooManyHeaders) => continue,
                Err(_) => {
                    self.state = FrameState::Broken;
                    return Err(FrameError::BadResponse);
                }
            }
        }
        self.state = FrameState::Broken;
        Err(FrameError::BadResponse)
    }

    /// Consumes a Content-Length body.
    fn frame_body(&mut self) -> Option<Response> {
        let FrameState::Body { remaining } = &self.state else {
            return None;
        };
        let remaining = *remaining;
        if self.buf.len() - self.scanned < remaining {
            return None;
        }
        if self.capture {
            self.body
                .extend_from_slice(&self.buf[self.scanned..self.scanned + remaining]);
        }
        self.scanned += remaining;
        Some(self.finish())
    }

    /// Consumes a chunked body.
    /// Consumes a chunked body.
    fn frame_chunked(&mut self) -> Result<Option<Response>, FrameError> {
        loop {
            match &self.state {
                FrameState::Chunked(ChunkState::Size) => {
                    let Some(line) = self.complete_line() else {
                        return Ok(None);
                    };
                    let text =
                        String::from_utf8_lossy(&self.buf[self.scanned..self.scanned + line])
                            .to_string();
                    let size = match usize::from_str_radix(text.trim(), 16) {
                        Ok(size) => size,
                        Err(_) => {
                            self.state = FrameState::Broken;
                            return Err(FrameError::BadResponse);
                        }
                    };
                    self.scanned += line + 2;
                    if size == 0 {
                        self.state = FrameState::Chunked(ChunkState::Trailers);
                    } else {
                        self.state = FrameState::Chunked(ChunkState::Data { remaining: size });
                    }
                }
                FrameState::Chunked(ChunkState::Data { remaining }) => {
                    let remaining = *remaining;
                    // Chunk data plus the trailing CRLF.
                    if self.buf.len() - self.scanned < remaining + 2 {
                        return Ok(None);
                    }
                    if self.capture {
                        self.body
                            .extend_from_slice(&self.buf[self.scanned..self.scanned + remaining]);
                    }
                    self.scanned += remaining + 2;
                    self.state = FrameState::Chunked(ChunkState::Size);
                }
                FrameState::Chunked(ChunkState::Trailers) => {
                    let Some(line) = self.complete_line() else {
                        return Ok(None);
                    };
                    // Trailer lines end with CRLF, an empty line closes
                    // the message.
                    self.scanned += line + 2;
                    if line == 0 {
                        return Ok(Some(self.finish()));
                    }
                }
                _ => return Ok(None),
            }
        }
    }

    /// Finds a complete CRLF line, returning its length without the
    /// terminator.
    fn complete_line(&self) -> Option<usize> {
        let line = self.buf[self.scanned..]
            .iter()
            .position(|byte| *byte == b'\r')?;
        (self.scanned + line + 2 <= self.buf.len()).then_some(line)
    }

    fn finish(&mut self) -> Response {
        self.state = FrameState::Headers;
        Response {
            status: self.status,
            headers: std::mem::take(&mut self.headers),
            body: std::mem::take(&mut self.body),
            keep_alive: self.message_keep_alive,
        }
    }

    /// Drops consumed bytes from the front of the buffer.
    fn compact(&mut self) {
        if self.scanned > 8192 {
            self.buf.drain(..self.scanned);
            self.scanned = 0;
        }
    }
}

/// Framing failures, every one reconnects like a parser error in C.
#[derive(Debug, PartialEq)]
pub enum FrameError {
    /// The bytes are not a response.
    BadResponse,
    /// The stream closed inside a message.
    ClosedMidMessage,
    /// The framer already failed, the connection is dead.
    Broken,
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

    use super::verify_request;

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: h\r\n\r\n")
    }

    #[test]
    fn verifies_single_requests() {
        assert_eq!(verify_request(get("/").as_bytes()), Ok(1));
    }

    #[test]
    fn counts_pipelined_requests() {
        let pipeline = get("/?foo") + &get("/?bar") + &get("/?baz");
        assert_eq!(verify_request(pipeline.as_bytes()), Ok(3));
    }

    #[test]
    fn frames_bodies_through_content_length() {
        let post = "POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello";
        let pipeline = format!("{post}{}", get("/"));
        assert_eq!(verify_request(pipeline.as_bytes()), Ok(2));
    }

    #[test]
    fn truncated_bodies_read_as_incomplete() {
        let post = "POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 9\r\n\r\nshort";
        assert!(
            verify_request(post.as_bytes())
                .unwrap_err()
                .starts_with("incomplete request at")
        );
    }

    #[test]
    fn partial_headers_read_as_incomplete() {
        let partial = "GET / HTTP/1.1\r\nHost: h\r\n";
        // The walk consumes the whole prefix, two newlines put the
        // position on line three at the first column.
        assert_eq!(
            verify_request(partial.as_bytes()),
            Err("incomplete request at 3:1".to_owned())
        );
    }

    #[test]
    fn garbage_carries_the_position() {
        let garbage = "\nnot a request";
        assert_eq!(
            verify_request(garbage.as_bytes()),
            Err("invalid HTTP version at 1:1".to_owned())
        );
    }

    #[test]
    fn empty_requests_are_incomplete() {
        assert_eq!(
            verify_request(b""),
            Err("incomplete request at 1:1".to_owned())
        );
    }

    use super::{FrameError, ResponseFramer};

    fn full(status: u16, headers: &[(&str, &str)], body: &str) -> String {
        let mut response = format!("HTTP/1.1 {status} X\r\n");
        for (name, value) in headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str("\r\n");
        response.push_str(body);
        response
    }

    fn response_of(status: u16, headers: Vec<(&str, &str)>, body: &str) -> super::Response {
        super::Response {
            status,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
            keep_alive: true,
        }
    }

    #[test]
    fn frames_content_length_responses() {
        let mut framer = ResponseFramer::new(true);
        let completed = framer
            .feed(full(200, &[("Content-Length", "5")], "hello").as_bytes())
            .expect("valid response");
        assert_eq!(
            completed,
            vec![response_of(200, vec![("Content-Length", "5")], "hello")]
        );
    }

    #[test]
    fn spans_feeds_and_back_to_back_messages() {
        let mut framer = ResponseFramer::new(true);
        let wire = full(200, &[("Content-Length", "2")], "hi")
            + &full(404, &[("Content-Length", "3")], "bye");
        let half = wire.len() / 3;
        assert!(framer.feed(&wire.as_bytes()[..half]).unwrap().is_empty());
        let rest = framer.feed(&wire.as_bytes()[half..]).unwrap();
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].status, 200);
        assert_eq!(rest[1].status, 404);
    }

    #[test]
    fn frames_chunked_responses() {
        let mut framer = ResponseFramer::new(true);
        let wire = "HTTP/1.1 200 X\r\nTransfer-Encoding: chunked\r\n\r\n\
                    5\r\nhello\r\n1\r\n \r\n5\r\nworld\r\n0\r\n\r\n";
        let completed = framer.feed(wire.as_bytes()).unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].body, b"hello world");
        assert!(completed[0].keep_alive);
    }

    #[test]
    fn bodiless_statuses_frame_without_length() {
        let mut framer = ResponseFramer::new(true);
        let wire = "HTTP/1.1 204 No Content\r\n\r\n".repeat(2);
        let completed = framer.feed(wire.as_bytes()).unwrap();
        assert_eq!(completed.len(), 2);
        assert!(completed.iter().all(|response| response.body.is_empty()));
    }

    #[test]
    fn informational_responses_carry_no_body() {
        let mut framer = ResponseFramer::new(true);
        let wire = format!(
            "HTTP/1.1 100 Continue\r\n\r\n{}",
            full(200, &[("Content-Length", "2")], "ok")
        );
        let completed = framer.feed(wire.as_bytes()).unwrap();
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].status, 100);
        assert_eq!(completed[1].status, 200);
    }

    #[test]
    fn duplicate_headers_survive_in_order() {
        let mut framer = ResponseFramer::new(true);
        let wire = "HTTP/1.1 200 X\r\nSet-Cookie: a\r\nSet-Cookie: b\r\nContent-Length: 0\r\n\r\n";
        let completed = framer.feed(wire.as_bytes()).unwrap();
        assert_eq!(
            completed[0].headers,
            vec![
                ("Set-Cookie".to_owned(), "a".to_owned()),
                ("Set-Cookie".to_owned(), "b".to_owned()),
                ("Content-Length".to_owned(), "0".to_owned()),
            ]
        );
    }

    #[test]
    fn close_connections_complete_only_at_eof() {
        let mut framer = ResponseFramer::new(true);
        let wire = "HTTP/1.1 200 X\r\nConnection: close\r\n\r\nbody until the end";
        assert!(framer.feed(wire.as_bytes()).unwrap().is_empty());
        let completed = framer.closed().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].body, b"body until the end");
        assert!(!completed[0].keep_alive);
    }

    #[test]
    fn http_ten_closes_without_keep_alive() {
        let mut framer = ResponseFramer::new(true);
        let wire = "HTTP/1.0 200 X\r\nContent-Length: 2\r\n\r\nok";
        let completed = framer.feed(wire.as_bytes()).unwrap();
        assert_eq!(completed.len(), 1);
        assert!(!completed[0].keep_alive);
    }

    #[test]
    fn eof_inside_a_body_fails() {
        let mut framer = ResponseFramer::new(true);
        framer
            .feed(full(200, &[("Content-Length", "9")], "short").as_bytes())
            .unwrap();
        assert_eq!(framer.closed().unwrap_err(), FrameError::ClosedMidMessage);
    }

    #[test]
    fn garbage_fails_the_stream() {
        let mut framer = ResponseFramer::new(true);
        assert_eq!(
            framer.feed(b"not a response").unwrap_err(),
            FrameError::BadResponse
        );
    }

    #[test]
    fn uncaptured_responses_stay_empty() {
        let mut framer = ResponseFramer::new(false);
        let completed = framer
            .feed(full(200, &[("Content-Length", "5")], "hello").as_bytes())
            .unwrap();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].headers.is_empty());
        assert!(completed[0].body.is_empty());
    }
}
