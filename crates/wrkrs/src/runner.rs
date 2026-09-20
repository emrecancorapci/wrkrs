//! Orchestrates engines for a run: spec building, resolution, thread
//! setup, and the thread zero probes.

use wrkrs_engine::ScriptSpec;

use crate::cli::Config;

/// Builds the engine spec from the run configuration.
pub fn build_spec(config: &Config) -> ScriptSpec {
    ScriptSpec {
        url: config.url.clone(),
        parts: config.parts.clone(),
        script: config.script.clone(),
        headers: split_headers(&config.headers),
    }
}

/// Splits raw header arguments at the first colon followed by a space.
///
/// Headers without that separator drop silently, matching the
/// script.c loop over `-H` values.
fn split_headers(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|header| {
            let colon = header.find(':')?;
            let split = header.as_bytes().get(colon + 1) == Some(&b' ');
            split.then(|| {
                let name = header[..colon].to_owned();
                let value = header[colon + 2..].to_owned();
                (name, value)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{build_spec, split_headers};
    use crate::cli::Config;
    use crate::parser::parse_url;

    fn config(headers: &[&str]) -> Config {
        Config {
            threads: 2,
            connections: 10,
            duration_s: 10,
            timeout_ms: 2000,
            latency: false,
            script: None,
            headers: headers.iter().map(|header| (*header).to_owned()).collect(),
            engine: None,
            url: "http://host/".to_owned(),
            parts: parse_url("http://host/").expect("valid url"),
            init_args: vec!["http://host/".to_owned()],
        }
    }

    #[test]
    fn splits_headers_at_the_first_separator() {
        let split = split_headers(&[
            "Accept: application/json".to_owned(),
            "X-Weird: A: B".to_owned(),
        ]);
        assert_eq!(
            split,
            vec![
                ("Accept".to_owned(), "application/json".to_owned()),
                ("X-Weird".to_owned(), "A: B".to_owned()),
            ]
        );
    }

    #[test]
    fn headers_without_the_separator_drop() {
        assert!(split_headers(&["Broken".to_owned()]).is_empty());
        assert!(split_headers(&["A:B".to_owned()]).is_empty());
        // An empty name still passes, the C loop keeps it.
        assert_eq!(
            split_headers(&[": value".to_owned()]),
            vec![("".to_owned(), "value".to_owned())]
        );
    }

    #[test]
    fn the_spec_carries_the_url_parts_and_script() {
        let config = config(&[]);
        let spec = build_spec(&config);
        assert_eq!(spec.url, "http://host/");
        assert_eq!(spec.parts.path, "/");
        assert!(spec.script.is_none());
        assert!(spec.headers.is_empty());
    }
}
