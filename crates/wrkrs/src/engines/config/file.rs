//! The benchmark file format, TOML and JSON shapes of one schema.

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::template::Template;

/// One parsed benchmark file.
///
/// The TOML shape:
///
/// ```toml
/// [request]
/// method = "POST"
/// path = "/api"
/// body = "foo=bar"
///
/// [request.headers]
/// Content-Type = "application/x-www-form-urlencoded"
///
/// [capture]
/// header = "X-Token"
/// path = "/resource"
///
/// [stop]
/// requests = 1000
/// ```
///
/// The JSON shape carries the same fields as nested objects. Absent
/// sections mean the defaults.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchFile {
    /// Request shaping, absent means the wrk defaults.
    #[serde(default)]
    pub request: RequestSpec,
    /// One capture rule, absent means no capture.
    #[serde(default)]
    pub capture: Option<CaptureSpec>,
    /// Stop conditions, absent means run the full duration.
    #[serde(default)]
    pub stop: StopSpec,
}

/// The `[request]` section.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestSpec {
    /// HTTP method, absent means GET.
    pub method: Option<String>,
    /// Request path, query included, absent means the URL path.
    pub path: Option<String>,
    /// Request body, absent means none. An empty string is a present
    /// body of length zero.
    pub body: Option<String>,
    /// Headers layered over the `-H` values, document order.
    #[serde(default, deserialize_with = "ordered_pairs")]
    pub headers: Vec<(String, String)>,
}

/// The `[capture]` section.
///
/// The auth.lua flow as data: while no response carried the header
/// yet, every request goes to the `[request]` path, the first
/// response that carries it switches the run to `path` with the
/// value set as the `to` request header.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSpec {
    /// The response header to copy, required.
    pub header: String,
    /// The path of the requests after the capture, absent keeps the
    /// `[request]` path.
    pub path: Option<String>,
    /// The request header that carries the captured value, absent
    /// means the same name as `header`.
    pub to: Option<String>,
}

/// The `[stop]` section.
///
/// Limits count per thread, the stop.lua behavior as fields.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopSpec {
    /// Stop after this many completed requests.
    pub requests: Option<u64>,
    /// Stop after this many response body bytes.
    pub bytes: Option<u64>,
}

impl StopSpec {
    /// Whether a thread that completed this many requests and
    /// received this many body bytes crossed a limit.
    pub fn reached(&self, completed: u64, received: u64) -> bool {
        self.requests.is_some_and(|limit| completed >= limit)
            || self.bytes.is_some_and(|limit| received >= limit)
    }

    /// Whether any stop condition is set.
    pub fn is_set(&self) -> bool {
        self.requests.is_some() || self.bytes.is_some()
    }
}

/// An error while opening or parsing a benchmark file.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// The file could not be read.
    #[error("cannot open {0}: {1}")]
    Open(String, std::io::Error),
    /// The file is not a valid benchmark file.
    #[error("{0}: {1}")]
    Parse(String, String),
}

/// Parses a benchmark file, JSON for the `.json` extension, TOML
/// otherwise.
///
/// A data file is not a script, so malformed input is a hard error
/// instead of the continue with defaults behavior script load
/// failures keep.
pub fn parse(path: &Path) -> Result<BenchFile, FileError> {
    let name = path.display().to_string();
    let source = fs::read_to_string(path).map_err(|error| FileError::Open(name, error))?;
    let parsed: Result<BenchFile, String> =
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => serde_json::from_str(&source).map_err(|error| error.to_string()),
            _ => toml::from_str(&source).map_err(|error| error.to_string()),
        };
    let file = parsed.map_err(|error| FileError::Parse(path.display().to_string(), error))?;
    file.validate()
        .map_err(|message| FileError::Parse(path.display().to_string(), message))?;
    Ok(file)
}

impl BenchFile {
    /// Whether any request field carries a template tag.
    ///
    /// A templated file renders per burst instead of once, the
    /// dynamic half of the config engine.
    pub fn is_templated(&self) -> bool {
        self.request.path.as_deref().is_some_and(Template::detect)
            || self.request.body.as_deref().is_some_and(Template::detect)
            || self
                .request
                .headers
                .iter()
                .any(|(_, value)| Template::detect(value))
    }

    /// Whether the file renders per burst, templated fields or a
    /// capture rule.
    pub fn is_dynamic(&self) -> bool {
        self.is_templated() || self.capture.is_some()
    }

    /// Rejects values that would corrupt the wire format.
    ///
    /// A script can produce whatever bytes it wants, a data file
    /// cannot, so fields that shape request lines and headers are
    /// checked before a run starts.
    pub fn validate(&self) -> Result<(), String> {
        let request = &self.request;
        if let Some(method) = &request.method
            && (method.is_empty() || method.bytes().any(|byte| byte <= b' ' || byte == 0x7f))
        {
            return Err(format!("invalid method '{method}'"));
        }
        if let Some(method) = &request.method
            && Template::detect(method)
        {
            return Err(format!(
                "invalid method '{method}', the method is not a template field"
            ));
        }
        if let Some(path) = &request.path {
            check_path("path", path)?;
        }
        for (name, value) in &request.headers {
            if !is_header_name(name) {
                return Err(format!("invalid header name '{name}'"));
            }
            if Template::detect(name) {
                return Err(format!(
                    "invalid header name '{name}', header names are not template fields"
                ));
            }
            if value.bytes().any(|byte| byte < b' ' || byte == 0x7f)
                || value.starts_with(' ')
                || value.ends_with(' ')
            {
                return Err(format!("invalid header value for '{name}'"));
            }
        }
        if let Some(capture) = &self.capture {
            if !is_header_name(&capture.header) {
                return Err(format!("invalid capture header '{}'", capture.header));
            }
            if Template::detect(&capture.header) {
                return Err(format!(
                    "invalid capture header '{}', the capture header is not a template field",
                    capture.header
                ));
            }
            if let Some(to) = &capture.to {
                if !is_header_name(to) {
                    return Err(format!("invalid capture target '{to}'"));
                }
                if Template::detect(to) {
                    return Err(format!(
                        "invalid capture target '{to}', the capture target is not a template field"
                    ));
                }
            }
            if let Some(path) = &capture.path {
                check_path("capture path", path)?;
            }
        }
        // Tags must name a known value, a typo would otherwise ride
        // the wire as literal text.
        for (field, value) in [
            ("request.path", request.path.as_deref()),
            ("request.body", request.body.as_deref()),
            (
                "capture.path",
                self.capture.as_ref().and_then(|c| c.path.as_deref()),
            ),
        ] {
            if let Some(value) = value {
                Template::new(value)
                    .map_err(|error| format!("invalid template in {field}: {error}"))?;
            }
        }
        for (name, value) in &request.headers {
            Template::new(value)
                .map_err(|error| format!("invalid template in request.headers.{name}: {error}"))?;
        }
        for (field, limit) in [("requests", self.stop.requests), ("bytes", self.stop.bytes)] {
            if limit.is_some_and(|limit| limit == 0) {
                return Err(format!("stop.{field} must be at least one"));
            }
        }
        Ok(())
    }
}

/// Header name rules, shared by request headers and capture names.
fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && !name
            .bytes()
            .any(|byte| byte <= b' ' || byte == b':' || byte == 0x7f)
}

/// Path rules, shared by the request path and the capture path.
fn check_path(field: &str, path: &str) -> Result<(), String> {
    if path != "*" && !path.starts_with('/') {
        return Err(format!(
            "invalid {field} '{path}', the path must start with a slash"
        ));
    }
    if path.bytes().any(|byte| byte <= b' ' || byte == 0x7f) {
        return Err(format!("invalid {field} '{path}'"));
    }
    Ok(())
}

/// Collects a string to string map as pairs in document order.
///
/// TOML tables and JSON objects feed entries in file order, which
/// keeps header order the order the file writes it in.
fn ordered_pairs<'de, D>(deserializer: D) -> Result<Vec<(String, String)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct PairsVisitor;

    impl<'de> serde::de::Visitor<'de> for PairsVisitor {
        type Value = Vec<(String, String)>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a table of names to string values")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
            while let Some((name, value)) = map.next_entry::<String, String>()? {
                pairs.push((name, value));
            }
            Ok(pairs)
        }
    }

    deserializer.deserialize_map(PairsVisitor)
}

#[cfg(test)]
mod tests {
    use super::{BenchFile, FileError, parse};

    fn from_toml(source: &str) -> BenchFile {
        toml::from_str(source).expect("toml parses")
    }

    #[test]
    fn empty_file_means_all_defaults() {
        let file = from_toml("");
        assert_eq!(file, BenchFile::default());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn json_empty_object_means_all_defaults() {
        let file: BenchFile = serde_json::from_str("{}").expect("json parses");
        assert_eq!(file, BenchFile::default());
    }

    #[test]
    fn parses_the_full_request_section() {
        let file = from_toml(
            "[request]\nmethod = \"POST\"\npath = \"/api?key=1\"\nbody = \"a=b\"\n\
             [request.headers]\nContent-Type = \"text/plain\"\nX-Api = \"2\"\n",
        );
        assert_eq!(file.request.method.as_deref(), Some("POST"));
        assert_eq!(file.request.path.as_deref(), Some("/api?key=1"));
        assert_eq!(file.request.body.as_deref(), Some("a=b"));
        assert_eq!(
            file.request.headers,
            vec![
                ("Content-Type".to_owned(), "text/plain".to_owned()),
                ("X-Api".to_owned(), "2".to_owned()),
            ]
        );
        assert_eq!(file.stop, super::StopSpec::default());
    }

    #[test]
    fn json_carries_the_same_shape() {
        let file: BenchFile = serde_json::from_str(
            "{\"request\":{\"method\":\"POST\",\"body\":\"a=b\",\
             \"headers\":{\"Content-Type\":\"text/plain\",\"X-Api\":\"2\"}},\
             \"stop\":{\"requests\":100}}",
        )
        .expect("json parses");
        assert_eq!(file.request.method.as_deref(), Some("POST"));
        assert_eq!(
            file.request.headers,
            vec![
                ("Content-Type".to_owned(), "text/plain".to_owned()),
                ("X-Api".to_owned(), "2".to_owned()),
            ]
        );
        assert_eq!(file.stop.requests, Some(100));
    }

    #[test]
    fn preserves_header_document_order() {
        let file =
            from_toml("[request.headers]\nZed = \"1\"\nAlpha = \"2\"\nMid = \"3\"\nBeta = \"4\"\n");
        let names: Vec<&str> = file
            .request
            .headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["Zed", "Alpha", "Mid", "Beta"]);
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(toml::from_str::<BenchFile>("[nope]\nx = 1").is_err());
        assert!(toml::from_str::<BenchFile>("[request]\nnope = 1").is_err());
        assert!(toml::from_str::<BenchFile>("[stop]\nnope = 1").is_err());
        assert!(serde_json::from_str::<BenchFile>("{\"nope\":1}").is_err());
    }

    #[test]
    fn rejects_wrong_types() {
        assert!(toml::from_str::<BenchFile>("[request]\nmethod = 7").is_err());
        assert!(toml::from_str::<BenchFile>("[stop]\nrequests = \"many\"").is_err());
        assert!(toml::from_str::<BenchFile>("[request]\nbody = [1, 2]").is_err());
        assert!(serde_json::from_str::<BenchFile>("{\"stop\":{\"requests\":-1}}").is_err());
    }

    #[test]
    fn rejects_non_string_header_values() {
        assert!(toml::from_str::<BenchFile>("[request.headers]\nX-Api = 2").is_err());
        assert!(toml::from_str::<BenchFile>("[request.headers]\nX-Api = true").is_err());
        assert!(
            serde_json::from_str::<BenchFile>("{\"request\":{\"headers\":{\"X-Api\":2}}}").is_err()
        );
    }

    #[test]
    fn rejects_headers_that_are_not_a_table() {
        assert!(toml::from_str::<BenchFile>("[request]\nheaders = [\"a\"]").is_err());
        assert!(serde_json::from_str::<BenchFile>("{\"request\":{\"headers\":[\"a\"]}}").is_err());
        assert!(
            serde_json::from_str::<BenchFile>("{\"request\":{\"headers\":\"text/plain\"}}")
                .is_err()
        );
    }

    #[test]
    fn json_duplicate_header_names_keep_the_last_value() {
        let file: BenchFile = serde_json::from_str(
            "{\"request\":{\"headers\":{\"X-Api\":\"first\",\"X-Api\":\"second\"}}}",
        )
        .expect("json parses");
        let values: Vec<&str> = file
            .request
            .headers
            .iter()
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(values, ["first", "second"]);
    }

    #[test]
    fn validates_the_method() {
        let mut file = BenchFile::default();
        for method in ["", "GET POST", "GE\tT"] {
            file.request.method = Some(method.to_owned());
            assert!(file.validate().is_err(), "method '{method}' passed");
        }
        file.request.method = Some("POST".to_owned());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn validates_the_path() {
        let mut file = BenchFile::default();
        file.request.path = Some("api/key".to_owned());
        assert!(file.validate().is_err());
        file.request.path = Some("/api key".to_owned());
        assert!(file.validate().is_err());
        file.request.path = Some("/api?key=1".to_owned());
        assert!(file.validate().is_ok());
        file.request.path = Some("*".to_owned());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn validates_headers() {
        let mut file = BenchFile::default();
        file.request.headers = vec![("Bad Name".to_owned(), "1".to_owned())];
        assert!(file.validate().is_err());
        file.request.headers = vec![("X-Bad".to_owned(), "a\r\nb".to_owned())];
        assert!(file.validate().is_err());
        file.request.headers = vec![("X-Bad".to_owned(), " lead".to_owned())];
        assert!(file.validate().is_err());
        file.request.headers = vec![("X-Ok".to_owned(), String::new())];
        assert!(file.validate().is_ok());
    }

    #[test]
    fn rejects_zero_limits() {
        let mut file = BenchFile::default();
        file.stop.requests = Some(0);
        assert_eq!(
            file.validate().unwrap_err(),
            "stop.requests must be at least one"
        );
        file.stop.requests = None;
        file.stop.bytes = Some(0);
        assert_eq!(
            file.validate().unwrap_err(),
            "stop.bytes must be at least one"
        );
    }

    #[test]
    fn stop_limits_reach_and_flag() {
        let mut stop = super::StopSpec::default();
        assert!(!stop.is_set());
        stop.requests = Some(3);
        assert!(stop.is_set());
        assert!(!stop.reached(2, 0));
        assert!(stop.reached(3, 0));
        stop.bytes = Some(10);
        assert!(stop.reached(0, 10));
        assert!(!super::StopSpec::default().reached(u64::MAX, u64::MAX));
    }

    #[test]
    fn templated_fields_flag_the_file() {
        let file = from_toml(
            "[request]\npath = \"/items/{{n}}\"\nbody = \"n={{n}}&r={{rand}}\"\n\
             [request.headers]\nX-Counter = \"{{n}}\"\n",
        );
        assert!(file.is_templated());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn plain_files_stay_literal() {
        let file = from_toml(
            "[request]\npath = \"/items/1\"\nbody = \"n=1\"\n\
             [request.headers]\nX-Counter = \"one\"\n",
        );
        assert!(!file.is_templated());
    }

    #[test]
    fn json_files_carry_templates_too() {
        let file: BenchFile =
            serde_json::from_str("{\"request\":{\"path\":\"/{{n}}\"}}").expect("json parses");
        assert!(file.is_templated());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn unknown_tags_fail_validation_with_the_field() {
        let mut file = BenchFile::default();
        file.request.path = Some("/x/{{count}}".to_owned());
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid template in request.path: \
             unknown template tag '{{count}}', known tags are {{n}} and {{rand}}"
        );
        file.request.path = Some("/x/{{n".to_owned());
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid template in request.path: \
             unterminated template tag '{{n', known tags are {{n}} and {{rand}}"
        );
        file.request.path = None;
        file.request.body = Some("{{ n }}".to_owned());
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid template in request.body: \
             unknown template tag '{{ n }}', known tags are {{n}} and {{rand}}"
        );
        file.request.body = None;
        file.request.headers = vec![("X-Api".to_owned(), "{{rand}".to_owned())];
        assert!(
            file.validate()
                .unwrap_err()
                .contains("invalid template in request.headers.X-Api: unterminated")
        );
    }

    #[test]
    fn methods_and_header_names_never_carry_tags() {
        let mut file = BenchFile::default();
        file.request.method = Some("{{n}}".to_owned());
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid method '{{n}}', the method is not a template field"
        );
        file.request.method = None;
        file.request.headers = vec![("{{n}}".to_owned(), "1".to_owned())];
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid header name '{{n}}', header names are not template fields"
        );
    }

    #[test]
    fn parses_the_capture_section() {
        let file = from_toml(
            "[request]\npath = \"/authenticate\"\n[capture]\nheader = \"X-Token\"\n\
             path = \"/resource\"\nto = \"Authorization\"\n",
        );
        let capture = file.capture.as_ref().expect("the capture section");
        assert_eq!(capture.header, "X-Token");
        assert_eq!(capture.path.as_deref(), Some("/resource"));
        assert_eq!(capture.to.as_deref(), Some("Authorization"));
        assert!(file.is_dynamic());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn capture_path_and_to_are_optional() {
        let file = from_toml("[capture]\nheader = \"Set-Cookie\"\n");
        let capture = file.capture.as_ref().expect("the capture section");
        assert_eq!(capture.path, None);
        assert_eq!(capture.to, None);
        assert!(file.is_dynamic());
        assert!(file.validate().is_ok());
    }

    #[test]
    fn json_files_carry_the_capture_section() {
        let file: BenchFile = serde_json::from_str(
            "{\"request\":{\"path\":\"/authenticate\"},\
             \"capture\":{\"header\":\"X-Token\",\"path\":\"/resource\"}}",
        )
        .expect("json parses");
        assert_eq!(file.capture.as_ref().expect("the rule").header, "X-Token");
        assert!(file.is_dynamic());
    }

    #[test]
    fn capture_rejects_unknown_fields_and_a_missing_header() {
        assert!(toml::from_str::<BenchFile>("[capture]\nnope = 1").is_err());
        assert!(toml::from_str::<BenchFile>("[capture]\npath = \"/resource\"").is_err());
        assert!(serde_json::from_str::<BenchFile>("{\"capture\":{}}").is_err());
        assert!(toml::from_str::<BenchFile>("[capture]\nheader = 7").is_err());
    }

    #[test]
    fn capture_names_and_path_validate() {
        let mut file = from_toml("[capture]\nheader = \"X-Token\"\n");
        file.capture.as_mut().expect("the rule").header = "Bad Name".to_owned();
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid capture header 'Bad Name'"
        );
        file.capture.as_mut().expect("the rule").header = "{{n}}".to_owned();
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid capture header '{{n}}', the capture header is not a template field"
        );
        let mut file = from_toml("[capture]\nheader = \"X-Token\"\n");
        file.capture.as_mut().expect("the rule").to = Some("Bad Name".to_owned());
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid capture target 'Bad Name'"
        );
        let mut file = from_toml("[capture]\nheader = \"X-Token\"\n");
        file.capture.as_mut().expect("the rule").path = Some("resource".to_owned());
        assert_eq!(
            file.validate().unwrap_err(),
            "invalid capture path 'resource', the path must start with a slash"
        );
        let file = from_toml("[capture]\nheader = \"X-Token\"\npath = \"/item/{{n}}\"\n");
        assert!(file.validate().is_ok());
        let file = from_toml("[capture]\nheader = \"X-Token\"\npath = \"/item/{{count}}\"\n");
        assert!(
            file.validate()
                .unwrap_err()
                .contains("invalid template in capture.path")
        );
    }

    #[test]
    fn parse_dispatches_by_extension() {
        let toml_file = std::env::temp_dir().join("wrkrs-config-parse.toml");
        std::fs::write(&toml_file, "[stop]\nrequests = 5\n").expect("write temp file");
        assert_eq!(parse(&toml_file).unwrap().stop.requests, Some(5));

        let json_file = std::env::temp_dir().join("wrkrs-config-parse.json");
        std::fs::write(&json_file, "{\"stop\":{\"requests\":6}}").expect("write temp file");
        assert_eq!(parse(&json_file).unwrap().stop.requests, Some(6));
    }

    #[test]
    fn parse_reports_the_file_name() {
        let missing = std::env::temp_dir().join("wrkrs-config-missing.toml");
        let _ = std::fs::remove_file(&missing);
        let error = parse(&missing).unwrap_err();
        assert!(matches!(error, FileError::Open(_, _)));
        assert!(error.to_string().contains("wrkrs-config-missing.toml"));

        let broken = std::env::temp_dir().join("wrkrs-config-broken.toml");
        std::fs::write(&broken, "[request]\nmethod = ").expect("write temp file");
        let error = parse(&broken).unwrap_err();
        assert!(matches!(error, FileError::Parse(_, _)));

        let invalid = std::env::temp_dir().join("wrkrs-config-invalid.toml");
        std::fs::write(&invalid, "[request]\npath = \"no-slash\"\n").expect("write temp file");
        let error = parse(&invalid).unwrap_err();
        assert!(error.to_string().contains("must start with a slash"));

        let mistyped = std::env::temp_dir().join("wrkrs-config-mistyped.toml");
        std::fs::write(&mistyped, "[request]\npath = \"/x/{{count}}\"\n").expect("write temp file");
        let error = parse(&mistyped).unwrap_err();
        assert!(matches!(error, FileError::Parse(_, _)));
        assert!(error.to_string().contains("unknown template tag"));
    }

    #[test]
    fn parse_rejects_non_utf8_files() {
        let binary = std::env::temp_dir().join("wrkrs-config-binary.toml");
        std::fs::write(&binary, [0x50, 0x4f, 0x53, 0x54, 0xff, 0xfe]).expect("write temp file");
        let error = parse(&binary).unwrap_err();
        assert!(matches!(error, FileError::Open(_, _)));
    }
}
