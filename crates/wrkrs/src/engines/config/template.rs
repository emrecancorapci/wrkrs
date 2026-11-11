//! Template tags inside benchmark file string fields.
//!
//! Light templating covers the shapes a data file needs: `{{n}}`
//! renders the per thread counter, `{{rand}}` a random integer,
//! everything else stays literal. The tag set is closed and typos
//! are errors, a data file is not a language.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

/// The tag opening delimiter.
const OPEN: &str = "{{";
/// The tag closing delimiter.
const CLOSE: &str = "}}";
/// The tail every tag error explains the tag set with.
const KNOWN: &str = "known tags are {{n}} and {{rand}}";

/// One piece of a template string.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// Literal text, rendered verbatim.
    Literal(String),
    /// `{{n}}`, the per thread counter.
    Counter,
    /// `{{rand}}`, one fresh random integer per occurrence.
    Random,
}

/// A parsed template string, the literals with the tag positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    segments: Vec<Segment>,
}

impl Template {
    /// Whether the source carries a tag, fields without one stay
    /// literal.
    pub fn detect(source: &str) -> bool {
        source.contains(OPEN)
    }

    /// Builds a template that renders the source verbatim, for
    /// values that are not file fields and never carry tags.
    pub fn literal(source: &str) -> Template {
        Template {
            segments: vec![Segment::Literal(source.to_owned())],
        }
    }

    /// Parses a field value into segments.
    ///
    /// Every `{{` must open exactly `{{n}}` or `{{rand}}`, anything
    /// else is an error naming the tag, so a typo cannot ride the
    /// wire as literal text.
    pub fn new(source: &str) -> Result<Template, String> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut rest = source;
        while let Some(start) = rest.find(OPEN) {
            literal.push_str(&rest[..start]);
            let after = &rest[start + OPEN.len()..];
            let end = after
                .find(CLOSE)
                .ok_or_else(|| format!("unterminated template tag '{OPEN}{after}', {KNOWN}"))?;
            let segment = match &after[..end] {
                "n" => Segment::Counter,
                "rand" => Segment::Random,
                tag => {
                    return Err(format!(
                        "unknown template tag '{OPEN}{tag}{CLOSE}', {KNOWN}"
                    ));
                }
            };
            if !literal.is_empty() {
                segments.push(Segment::Literal(std::mem::take(&mut literal)));
            }
            segments.push(segment);
            rest = &after[end + CLOSE.len()..];
        }
        literal.push_str(rest);
        if !literal.is_empty() || segments.is_empty() {
            segments.push(Segment::Literal(literal));
        }
        Ok(Template { segments })
    }

    /// Renders the string, one counter value for the whole render,
    /// one fresh integer per random tag.
    pub fn render(&self, counter: u64, rng: &mut Rng) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Literal(text) => out.push_str(text),
                Segment::Counter => out.push_str(&counter.to_string()),
                Segment::Random => out.push_str(&rng.next_u64().to_string()),
            }
        }
        out
    }
}

/// The random source behind `{{rand}}`.
///
/// splitmix64 over a seed mixing the OS keyed hasher with a process
/// counter, so every engine instance draws an independent stream
/// without a full RNG dependency.
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seeds one independent stream.
    pub fn new() -> Rng {
        static SEEDS: AtomicU64 = AtomicU64::new(0);
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(SEEDS.fetch_add(1, Ordering::Relaxed));
        Rng {
            state: hasher.finish(),
        }
    }

    /// Seeds a fixed stream, tests only.
    #[cfg(test)]
    fn from_seed(seed: u64) -> Rng {
        Rng { state: seed }
    }

    /// Draws the next integer.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::{Rng, Template};

    #[test]
    fn plain_text_renders_verbatim() {
        let template = Template::new("/items/7?x=y").unwrap();
        assert_eq!(template.render(1, &mut Rng::from_seed(1)), "/items/7?x=y");
    }

    #[test]
    fn an_empty_source_is_an_empty_literal() {
        let template = Template::new("").unwrap();
        assert!(!Template::detect(""));
        let rendered = template.render(3, &mut Rng::from_seed(1));
        assert_eq!(rendered, "");
    }

    #[test]
    fn counter_tags_render_the_given_value() {
        let template = Template::new("/items/{{n}}").unwrap();
        assert_eq!(template.render(1, &mut Rng::from_seed(1)), "/items/1");
        assert_eq!(template.render(2, &mut Rng::from_seed(1)), "/items/2");
    }

    #[test]
    fn counter_tags_share_one_value_per_render() {
        let template = Template::new("{{n}}/{{n}}").unwrap();
        assert_eq!(template.render(7, &mut Rng::from_seed(1)), "7/7");
    }

    #[test]
    fn random_tags_render_digits_and_change_per_draw() {
        let template = Template::new("{{rand}}").unwrap();
        let mut rng = Rng::from_seed(1);
        let first = template.render(1, &mut rng);
        let second = template.render(1, &mut rng);
        assert!(first.bytes().all(|byte| byte.is_ascii_digit()));
        assert_ne!(first, second);
    }

    #[test]
    fn random_tags_draw_per_occurrence() {
        let template = Template::new("{{rand}},{{rand}}").unwrap();
        let mut rng = Rng::from_seed(1);
        let rendered = template.render(1, &mut rng);
        let (left, right) = rendered.split_once(',').expect("two draws");
        assert_ne!(left, right);
    }

    #[test]
    fn literal_and_tag_segments_mix() {
        let template = Template::new("/items/{{n}}?session={{rand}}#{{n}}").unwrap();
        let rendered = template.render(4, &mut Rng::from_seed(1));
        assert!(rendered.starts_with("/items/4?session="));
        assert!(rendered.ends_with("#4"));
    }

    #[test]
    fn braces_without_an_opening_tag_stay_literal() {
        let template = Template::new("a}}b{c").unwrap();
        assert!(!Template::detect("a}}b{c"));
        assert_eq!(template.render(1, &mut Rng::from_seed(1)), "a}}b{c");
    }

    #[test]
    fn a_trailing_closing_brace_after_a_tag_stays_literal() {
        let template = Template::new("{{n}}}").unwrap();
        assert_eq!(template.render(1, &mut Rng::from_seed(1)), "1}");
    }

    #[test]
    fn unknown_tags_are_errors_that_name_the_tag() {
        assert_eq!(
            Template::new("/x/{{count}}").unwrap_err(),
            "unknown template tag '{{count}}', known tags are {{n}} and {{rand}}"
        );
        assert_eq!(
            Template::new("/x/{{}}").unwrap_err(),
            "unknown template tag '{{}}', known tags are {{n}} and {{rand}}"
        );
        assert_eq!(
            Template::new("/x/{{ n }}").unwrap_err(),
            "unknown template tag '{{ n }}', known tags are {{n}} and {{rand}}"
        );
        assert_eq!(
            Template::new("/x/{{N}}").unwrap_err(),
            "unknown template tag '{{N}}', known tags are {{n}} and {{rand}}"
        );
    }

    #[test]
    fn unterminated_tags_are_errors_that_show_the_rest() {
        assert_eq!(
            Template::new("/x/{{n").unwrap_err(),
            "unterminated template tag '{{n', known tags are {{n}} and {{rand}}"
        );
        assert_eq!(
            Template::new("{{rand}} {{nope").unwrap_err(),
            "unterminated template tag '{{nope', known tags are {{n}} and {{rand}}"
        );
    }

    #[test]
    fn literal_templates_wrap_sources_with_braces() {
        let template = Template::literal("{{not-a-file-field}}");
        assert_eq!(
            template.render(1, &mut Rng::from_seed(1)),
            "{{not-a-file-field}}"
        );
    }

    #[test]
    fn independent_streams_rarely_agree() {
        let mut left = Rng::new();
        let mut right = Rng::new();
        let disagreements = (0..8)
            .filter(|_| left.next_u64() != right.next_u64())
            .count();
        assert!(disagreements > 0);
    }
}
