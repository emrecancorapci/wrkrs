//! The QuickJS engine conformance run.

use wrkrs_engine_tests::{Scripts, run};

const SCRIPTS: Scripts = Scripts {
    post: "wrk.method = \"POST\"\n\
           wrk.headers[\"Content-Type\"] = \"text/plain\"\n\
           wrk.body = \"hello\"\n",
    pipeline: "var req\n\
               function init(args) {\n\
               req = wrk.format(null, \"/?foo\") + wrk.format(null, \"/?bar\")\n\
               }\n\
               function request() { return req }\n",
    delay: "function delay() { return 125 }\n",
    response_capture: "var seen_status, seen_type, seen_dup, seen_body\n\
                       function response(status, headers, body) {\n\
                       seen_status = status\n\
                       seen_type = headers[\"Content-Type\"]\n\
                       seen_dup = headers[\"X-Dup\"]\n\
                       seen_body = body\n\
                       }\n",
    done_capture: "var seen_duration, seen_connect, seen_percentile, seen_length\n\
                   function done(summary, latency, requests) {\n\
                   seen_duration = summary.duration\n\
                   seen_connect = summary.errors.connect\n\
                   seen_percentile = latency.percentile(99.0)\n\
                   seen_length = requests.length\n\
                   }\n",
    setup_transfer: "function setup(thread) { thread.set(\"id\", 7) }\n",
    init_args: "var seen_first\n\
                function init(args) { seen_first = args[0] }\n",
    full: "function request() { return \"GET / HTTP/1.1\\r\\n\\r\\n\" }\n\
           function response(status, headers, body) {}\n\
           function delay() { return 0 }\n\
           function done(summary, latency, requests) {}\n",
    runtime_error: "function request() { throw new Error(\"boom\") }\n",
    syntax_error: "this is not javascript\n",
};

#[test]
fn quickjs_engine_conforms() {
    run("quickjs", super::factory, &SCRIPTS);
}
