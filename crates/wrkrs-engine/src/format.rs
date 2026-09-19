/// Formats the `Host` request header value the way wrk init does.
///
/// A host containing a colon is wrapped in square brackets so literal
/// IPv6 addresses stay valid. The port is appended when present.
pub fn host_header(host: &str, port: Option<&str>) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// Builds HTTP request bytes with wrk format semantics.
///
/// The behavior ports `wrk.format` from `src/wrk.lua` exactly:
///
/// - a header list without a `Host` entry gains one from `default_host`
///   when the default exists
/// - `Content-Length` reflects the body. A present body overwrites an
///   existing entry in place or appends one. An absent body removes the
///   entry. An empty body is present and yields length zero
/// - request line, headers, empty line, and body join with `\r\n` and
///   the result carries no trailing separator
///
/// Header order is preserved. Scripting VMs that drive this helper
/// through their own tables keep their native iteration order, which is
/// what wrk observably does.
///
/// The wrk Lua version also writes the `Host` and `Content-Length`
/// changes back into the header table it was given. That mutation stays
/// on the engine side, where the table lives.
pub fn format_request(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    default_host: Option<&str>,
) -> Vec<u8> {
    let mut effective = headers.to_vec();

    if let Some(host) = default_host
        && !effective.iter().any(|(name, _)| name == "Host")
    {
        effective.push(("Host".to_owned(), host.to_owned()));
    }

    apply_content_length(&mut effective, body.map(|body| body.len()));

    let mut request = Vec::new();
    request.extend_from_slice(format!("{method} {path} HTTP/1.1").as_bytes());
    for (name, value) in &effective {
        request.extend_from_slice(b"\r\n");
        request.extend_from_slice(format!("{name}: {value}").as_bytes());
    }
    request.extend_from_slice(b"\r\n\r\n");
    if let Some(body) = body {
        request.extend_from_slice(body);
    }
    request
}

/// Applies the wrk format `Content-Length` rule to a header list.
fn apply_content_length(headers: &mut Vec<(String, String)>, length: Option<usize>) {
    let entry = headers
        .iter_mut()
        .find(|(name, _)| name == "Content-Length");

    match (entry, length) {
        (Some(entry), Some(length)) => entry.1 = length.to_string(),
        (Some(_), None) => {
            headers.retain(|(name, _)| name != "Content-Length");
        }
        (None, Some(length)) => {
            headers.push(("Content-Length".to_owned(), length.to_string()));
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{format_request, host_header};

    fn headers(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn formats_default_get_request() {
        let request = format_request("GET", "/", &[], None, Some("localhost"));
        assert_eq!(request, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    }

    #[test]
    fn omits_host_when_default_is_missing() {
        let request = format_request("GET", "/", &[], None, None);
        assert_eq!(request, b"GET / HTTP/1.1\r\n\r\n");
    }

    #[test]
    fn keeps_custom_host_header() {
        let request = format_request(
            "GET",
            "/",
            &headers(&[("Accept", "*/*"), ("Host", "example.com")]),
            None,
            Some("localhost"),
        );
        assert_eq!(
            request,
            b"GET / HTTP/1.1\r\nAccept: */*\r\nHost: example.com\r\n\r\n"
        );
    }

    #[test]
    fn appends_host_before_content_length() {
        let request = format_request(
            "POST",
            "/submit",
            &[],
            Some(b"name=wrk".as_slice()),
            Some("localhost"),
        );
        assert_eq!(
            request,
            b"POST /submit HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\r\nname=wrk"
        );
    }

    #[test]
    fn writes_zero_length_for_empty_body() {
        let request = format_request("POST", "/", &[], Some(b""), None);
        assert_eq!(request, b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
    }

    #[test]
    fn updates_content_length_in_place() {
        let request = format_request(
            "POST",
            "/",
            &headers(&[("Content-Length", "999"), ("Accept", "*/*")]),
            Some(b"abc"),
            None,
        );
        assert_eq!(
            request,
            b"POST / HTTP/1.1\r\nContent-Length: 3\r\nAccept: */*\r\n\r\nabc"
        );
    }

    #[test]
    fn removes_content_length_without_body() {
        let request = format_request(
            "GET",
            "/",
            &headers(&[("Accept", "*/*"), ("Content-Length", "10")]),
            None,
            None,
        );
        assert_eq!(request, b"GET / HTTP/1.1\r\nAccept: */*\r\n\r\n");
    }

    #[test]
    fn copies_body_bytes_verbatim() {
        let body = b"line\r\nline\0bin";
        let request = format_request("PUT", "/data", &[], Some(body.as_slice()), None);
        let expected = b"PUT /data HTTP/1.1\r\nContent-Length: 14\r\n\r\nline\r\nline\0bin";
        assert_eq!(request, expected);
    }

    #[test]
    fn passes_method_and_path_through_untouched() {
        let request = format_request("HEAD", "/a b?q", &[], None, None);
        assert_eq!(request, b"HEAD /a b?q HTTP/1.1\r\n\r\n");
    }

    #[test]
    fn preserves_header_order() {
        let request = format_request(
            "GET",
            "/",
            &headers(&[("B", "2"), ("A", "1"), ("C", "3")]),
            None,
            None,
        );
        assert_eq!(request, b"GET / HTTP/1.1\r\nB: 2\r\nA: 1\r\nC: 3\r\n\r\n");
    }

    #[test]
    fn formats_plain_host_without_port() {
        assert_eq!(host_header("localhost", None), "localhost");
    }

    #[test]
    fn formats_host_with_port() {
        assert_eq!(host_header("localhost", Some("8080")), "localhost:8080");
    }

    #[test]
    fn brackets_ipv6_host() {
        assert_eq!(host_header("::1", Some("8080")), "[::1]:8080");
        assert_eq!(host_header("2001:db8::1", None), "[2001:db8::1]");
    }
}
