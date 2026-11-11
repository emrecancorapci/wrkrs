//! The HTML reporter, the JSON shape as one self contained page.
//!
//! The page renders from the same object the JSON mode serializes,
//! so every number is identical by construction, and the exact JSON
//! document rides along in a `script` tag for tooling. The styles
//! are inline, no script runs, and nothing loads, so the file works
//! offline from a double click.

use std::io;

use crate::json::{JsonReport, json_report};
use crate::report::{Reporter, RunReport};
use crate::stats::Histogram;
use crate::units::{format_binary, format_metric, format_time_us};

/// Bars in the latency distribution chart.
const BARS: usize = 48;

/// The chart viewBox width.
const CHART_W: f64 = 760.0;
/// The chart viewBox height.
const CHART_H: f64 = 260.0;
/// The chart padding: left, right, top, bottom.
const PAD_L: f64 = 46.0;
const PAD_R: f64 = 10.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 30.0;

/// The HTML output mode.
#[derive(Default)]
pub struct HtmlReporter;

impl Reporter for HtmlReporter {
    fn report(&self, out: &mut dyn io::Write, run: &RunReport) {
        let report = json_report(run);
        let page = render_page(&report, &run.latency);
        out.write_all(page.as_bytes()).expect("report writes");
    }
}

/// Renders the page for one report object and its latency
/// distribution.
///
/// The object is the machine half of the page, the histogram is
/// only the chart input, which the JSON shape does not carry.
pub(crate) fn render_page(report: &JsonReport, latency: &Histogram) -> String {
    let version = escape(report.version);
    let mut page = String::with_capacity(12 * 1024);

    page.push_str("<!DOCTYPE html>\n");
    page.push_str("<html lang=\"en\">\n");
    page.push_str("<head>\n");
    page.push_str("<meta charset=\"utf-8\">\n");
    page.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    page.push_str("<title>wrkrs report</title>\n");
    page.push_str("<style>\n");
    page.push_str(STYLE);
    page.push_str("</style>\n");
    page.push_str("</head>\n");
    page.push_str("<body>\n");
    page.push_str(&format!("<h1>wrkrs report <span>v{version}</span></h1>\n"));

    page.push_str("<section id=\"totals\">\n<h2>Totals</h2>\n<div class=\"cards\">\n");
    page.push_str(&card(&report.requests.to_string(), "requests"));
    page.push_str(&card(
        &format_time_us(report.duration_us as f64),
        "duration",
    ));
    page.push_str(&card(
        &format!("{}B", format_binary(report.bytes as f64)),
        "bytes read",
    ));
    page.push_str(&card(
        &format_metric(report.requests_per_s),
        "requests per second",
    ));
    page.push_str(&card(
        &format!("{}B", format_binary(report.bytes_per_s)),
        "bytes per second",
    ));
    page.push_str("</div>\n</section>\n");

    let stats = &report.latency;
    page.push_str("<section id=\"latency\">\n<h2>Latency</h2>\n<div class=\"cards\">\n");
    page.push_str(&card(&format_time_us(stats.avg), "avg"));
    page.push_str(&card(&format_time_us(stats.stdev), "stdev"));
    page.push_str(&card(&format_time_us(stats.min as f64), "min"));
    page.push_str(&card(&format_time_us(stats.max as f64), "max"));
    page.push_str(&card(&micros(stats.p50), "p50"));
    page.push_str(&card(&micros(stats.p75), "p75"));
    page.push_str(&card(&micros(stats.p90), "p90"));
    page.push_str(&card(&micros(stats.p99), "p99"));
    page.push_str("</div>\n");
    page.push_str(&latency_chart(stats, latency));
    page.push_str("</section>\n");

    let rate = &report.rate;
    page.push_str("<section id=\"rate\">\n<h2>Requests per second</h2>\n<div class=\"cards\">\n");
    page.push_str(&card(&format_metric(rate.avg), "avg"));
    page.push_str(&card(&format_metric(rate.stdev), "stdev"));
    page.push_str(&card(&format_metric(rate.min as f64), "min"));
    page.push_str(&card(&format_metric(rate.max as f64), "max"));
    page.push_str(&card(
        &format!("{:.2}%", rate.within_1_stdev),
        "within 1 stdev",
    ));
    page.push_str("</div>\n</section>\n");

    let errors = &report.errors;
    page.push_str("<section id=\"errors\">\n<h2>Errors</h2>\n<div class=\"cards\">\n");
    page.push_str(&card(&errors.connect.to_string(), "connect"));
    page.push_str(&card(&errors.read.to_string(), "read"));
    page.push_str(&card(&errors.write.to_string(), "write"));
    page.push_str(&card(&errors.timeout.to_string(), "timeout"));
    page.push_str(&card(&errors.status.to_string(), "status"));
    page.push_str("</div>\n</section>\n");

    page.push_str("<script type=\"application/json\" id=\"wrkrs-report\">\n");
    page.push_str(&json_blob(report));
    page.push_str("\n</script>\n");
    page.push_str("</body>\n</html>\n");
    page
}

/// Renders the latency distribution, log scaled bars over the
/// occupied histogram buckets with percentile marks.
///
/// Latency spans decades on real runs, so a linear axis would cram
/// every bar into the first pixels under the tail. The marks sit at
/// min, p50, p90, p99, and max.
fn latency_chart(stats: &crate::json::Stats, histogram: &Histogram) -> String {
    if histogram.count() == 0 {
        return "<p class=\"empty\">no samples</p>\n".to_owned();
    }

    let lo = (histogram.min() as f64).max(1.0).ln();
    let hi = (histogram.max() as f64).max(1.0).ln();
    // A single distinct value collapses the span, one bar carries
    // everything and the marks share the middle.
    let span = if hi > lo { hi - lo } else { 1.0 };
    let position = |value: u64| (((value.max(1)) as f64).ln() - lo) / span;

    let mut bars = [0u64; BARS];
    for slot in 0..histogram.popcount() {
        let (value, count) = histogram.value_at(slot);
        let index = (position(value) * BARS as f64) as usize;
        bars[index.min(BARS - 1)] += count;
    }

    // The marks, ascending, duplicates merged keeping the line.
    let mut marks: Vec<(u64, bool)> = vec![(stats.min, false)];
    for percentile in [stats.p50, stats.p90, stats.p99] {
        if let Some(value) = percentile
            && value > marks.last().expect("min seeds the marks").0
        {
            marks.push((value, true));
        }
    }
    if stats.max > marks.last().expect("marks exist").0 {
        marks.push((stats.max, false));
    }

    let plot_w = CHART_W - PAD_L - PAD_R;
    let plot_h = CHART_H - PAD_T - PAD_B;
    let bar_w = plot_w / BARS as f64;
    let peak = bars.iter().copied().max().unwrap_or(1) as f64;

    let mut svg = format!(
        "  <svg viewBox=\"0 0 {CHART_W} {CHART_H}\" role=\"img\" aria-label=\"latency distribution\">\n"
    );
    for step in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let y = PAD_T + plot_h - plot_h * step;
        svg.push_str(&format!(
            "    <line class=\"grid\" x1=\"{PAD_L:.2}\" y1=\"{y:.2}\" x2=\"{:.2}\" y2=\"{y:.2}\"/>\n",
            CHART_W - PAD_R
        ));
        svg.push_str(&format!(
            "    <text class=\"count\" x=\"{:.2}\" y=\"{y:.2}\">{}</text>\n",
            PAD_L - 6.0,
            count_label(peak * step)
        ));
    }
    for (index, count) in bars.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let height = *count as f64 / peak * plot_h;
        let x = PAD_L + index as f64 * bar_w;
        svg.push_str(&format!(
            "    <rect class=\"bar\" x=\"{x:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{height:.2}\"/>\n",
            PAD_T + plot_h - height,
            bar_w - 1.0
        ));
    }
    let baseline = PAD_T + plot_h;
    let last = marks.len() - 1;
    for (index, (value, line)) in marks.iter().enumerate() {
        let x = PAD_L + position(*value) * plot_w;
        if *line {
            svg.push_str(&format!(
                "    <line class=\"pct\" x1=\"{x:.2}\" y1=\"{PAD_T:.2}\" x2=\"{x:.2}\" y2=\"{baseline:.2}\"/>\n"
            ));
        }
        svg.push_str(&format!(
            "    <line class=\"axis\" x1=\"{x:.2}\" y1=\"{baseline:.2}\" x2=\"{x:.2}\" y2=\"{:.2}\"/>\n",
            baseline + 4.0
        ));
        let anchor = match index {
            0 => "start",
            index if index == last => "end",
            _ => "middle",
        };
        svg.push_str(&format!(
            "    <text class=\"tick\" x=\"{x:.2}\" y=\"{:.2}\" text-anchor=\"{anchor}\">{}</text>\n",
            CHART_H - 8.0,
            format_time_us(*value as f64)
        ));
    }
    svg.push_str("  </svg>\n");
    svg
}

/// Formats one latency percentile.
fn micros(value: Option<u64>) -> String {
    format_time_us(value.unwrap_or(0) as f64)
}

/// Formats a bucket count for the chart axis.
fn count_label(count: f64) -> String {
    if count < 850.0 {
        format!("{count:.0}")
    } else {
        format_metric(count)
    }
}

/// One stat card.
fn card(value: &str, label: &str) -> String {
    format!("<div class=\"card\"><b>{value}</b><span>{label}</span></div>\n")
}

/// Serializes the report object the JSON mode prints, made safe for
/// embedding: `</` cannot close the host tag from inside a JSON
/// string because the slash is escaped.
fn json_blob(report: &JsonReport) -> String {
    serde_json::to_string_pretty(report)
        .expect("the report object serializes")
        .replace("</", "<\\/")
}

/// Escapes the one string field the page interpolates.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page styles, one inline block, no assets.
const STYLE: &str = "\
:root { color-scheme: light dark }
body { margin: 0 auto; max-width: 50rem; padding: 2rem 1rem 4rem;
       font: 15px/1.5 system-ui, -apple-system, sans-serif }
h1 { margin: 0; font-size: 1.05rem; letter-spacing: .08em;
     text-transform: uppercase }
h1 span { color: #6b7280 }
h2 { margin: 1.9rem 0 .6rem; font-size: .75rem; letter-spacing: .08em;
     text-transform: uppercase; color: #6b7280;
     border-bottom: 1px solid #e5e7eb; padding-bottom: .35rem }
.cards { display: grid; grid-template-columns: repeat(auto-fit,
         minmax(8rem, 1fr)); gap: .6rem }
.card { border: 1px solid #e5e7eb; border-radius: .4rem;
        padding: .5rem .65rem }
.card b { display: block; font-size: 1.05rem }
.card span { font-size: .72rem; color: #6b7280 }
svg { display: block; width: 100%; height: auto; margin-top: .8rem }
.bar { fill: #2563eb }
.grid { stroke: #e5e7eb }
.axis { stroke: #9ca3af }
.pct { stroke: #9ca3af; stroke-dasharray: 3 3 }
.count { fill: #6b7280; font-size: 11px; text-anchor: end }
.tick { fill: #6b7280; font-size: 11px }
.empty { color: #6b7280 }
@media (prefers-color-scheme: dark) {
  body { background: #0f172a; color: #e5e7eb }
  h2 { color: #9ca3af; border-color: #1f2937 }
  .card { border-color: #1f2937 }
  .card span { color: #9ca3af }
  .bar { fill: #60a5fa }
  .grid { stroke: #1f2937 }
  .axis { stroke: #4b5563 }
  .pct { stroke: #4b5563 }
  .count { fill: #9ca3af }
  .tick { fill: #9ca3af }
}
";

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::HtmlReporter;
    use crate::json::json_report_with_version;
    use crate::report::{Reporter, RunReport};
    use crate::stats::Histogram;

    /// The synthetic run behind the golden page.
    fn run() -> RunReport {
        let latency = Arc::new(Histogram::new(2_000_000));
        for value in [
            34, 53, 77, 171, 500, 1190, 1200, 1500, 1500, 2000, 3400, 171_000,
        ] {
            latency.record(value);
        }
        let rate = Arc::new(Histogram::new(10_000_000));
        for value in [95_400, 98_700, 99_800, 100_100, 100_300, 105_900, 107_200] {
            rate.record(value);
        }
        RunReport {
            duration_us: 2_123_456,
            complete: 424_167,
            bytes: 16_800_000,
            errors: wrkrs_engine::ErrorCounts {
                connect: 0,
                read: 2,
                write: 0,
                timeout: 1,
                status: 7,
            },
            latency,
            rate,
        }
    }

    /// The page the golden file holds, pinned to a fixed version so
    /// releases do not touch it.
    fn golden_page(run: &RunReport) -> String {
        let report = json_report_with_version(run, "golden");
        super::render_page(&report, &run.latency)
    }

    /// Rewrites the golden file from the synthetic run. Regenerate
    /// with `cargo test -p wrkrs regenerate_html_golden -- --ignored`
    /// whenever the page layout changes on purpose.
    #[test]
    #[ignore]
    fn regenerate_html_golden() {
        let run = run();
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/golden_html.html"),
            golden_page(&run),
        )
        .expect("write the golden file");
    }

    #[test]
    fn matches_the_golden_page() {
        let run = run();
        assert_eq!(
            golden_page(&run),
            include_str!("../../../tools/golden_html.html")
        );
    }

    /// The HTML void elements, everything else must close.
    const VOID: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source",
        "track", "wbr",
    ];

    /// The tag name at the start of a token.
    fn name_of(token: &str) -> &str {
        let end = token
            .find(|character: char| !character.is_ascii_alphanumeric())
            .unwrap_or(token.len());
        &token[..end]
    }

    /// Walks the tags of a page and fails on anything unbalanced,
    /// the light parse the acceptance asks for.
    fn assert_balanced(page: &str) {
        let mut stack: Vec<&str> = Vec::new();
        for token in page.split('<').skip(1) {
            if token.starts_with('!') || token.starts_with('?') {
                continue;
            }
            if let Some(rest) = token.strip_prefix('/') {
                let name = name_of(rest);
                assert_eq!(stack.pop(), Some(name), "the </{name}> tag is unbalanced");
            } else {
                let name = name_of(token);
                let self_closed = token.trim_end().ends_with("/>");
                if !VOID.contains(&name) && !self_closed {
                    stack.push(name);
                }
            }
        }
        assert!(stack.is_empty(), "unclosed tags: {stack:?}");
    }

    #[test]
    fn parses_and_carries_the_json_shape() {
        let run = run();
        let page = golden_page(&run);
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert_balanced(&page);

        let marker = "<script type=\"application/json\" id=\"wrkrs-report\">\n";
        let start = page.find(marker).expect("the blob opens") + marker.len();
        let end = start + page[start..].find("</script>").expect("the blob closes");
        let blob = &page[start..end];
        let value: serde_json::Value = serde_json::from_str(blob).expect("valid json");
        assert_eq!(value["duration_us"], 2_123_456);
        assert_eq!(value["requests"], 424_167);
        assert_eq!(value["errors"]["read"], 2);
        assert_eq!(value["errors"]["status"], 7);
        assert_eq!(value["latency"]["p99"], 171_000);
        assert_eq!(value["version"], "golden");

        // The chart drew bars and percentile marks.
        assert!(page.matches("class=\"bar\"").count() > 1);
        assert!(page.contains("class=\"pct\""));
        assert!(page.matches("<section").count() == 4);
    }

    #[test]
    fn the_reporter_writes_the_page_with_the_real_version() {
        let mut out = Vec::new();
        HtmlReporter.report(&mut out, &run());
        let page = String::from_utf8(out).expect("utf8");
        assert!(page.starts_with("<!DOCTYPE html>"));
        assert!(page.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn an_empty_run_renders_the_empty_note() {
        let run = RunReport {
            duration_us: 1_000_000,
            complete: 0,
            bytes: 0,
            errors: Default::default(),
            latency: Arc::new(Histogram::new(100)),
            rate: Arc::new(Histogram::new(100)),
        };
        let report = json_report_with_version(&run, "golden");
        let page = super::render_page(&report, &run.latency);
        assert!(page.contains("no samples"));
        assert!(!page.contains("class=\"bar\""));
        assert_balanced(&page);
    }

    #[test]
    fn a_single_value_draws_one_bar() {
        let latency = Arc::new(Histogram::new(1_000));
        latency.record(750);
        let run = RunReport {
            duration_us: 1_000_000,
            complete: 1,
            bytes: 10,
            errors: Default::default(),
            latency,
            rate: Arc::new(Histogram::new(100)),
        };
        let report = json_report_with_version(&run, "golden");
        let page = super::render_page(&report, &run.latency);
        assert_eq!(page.matches("class=\"bar\"").count(), 1);
        assert_balanced(&page);
    }
}
