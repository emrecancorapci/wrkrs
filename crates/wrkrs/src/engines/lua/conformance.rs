//! The Lua engine conformance run.

use wrkrs_engine_tests::{Scripts, run};

const SCRIPTS: Scripts = Scripts {
    post: "wrk.method = \"POST\"\n\
           wrk.headers[\"Content-Type\"] = \"text/plain\"\n\
           wrk.body = \"hello\"\n",
    pipeline: "function init(args)\n\
               req = wrk.format(nil, \"/?foo\") .. wrk.format(nil, \"/?bar\")\n\
               end\n\
               function request() return req end\n",
    delay: "function delay() return 125 end\n",
    response_capture: "function response(status, headers, body)\n\
                       seen_status = status\n\
                       seen_type = headers[\"Content-Type\"]\n\
                       seen_dup = headers[\"X-Dup\"]\n\
                       seen_body = body\n\
                       end\n",
    done_capture: "function done(summary, latency, requests)\n\
                   seen_duration = summary.duration\n\
                   seen_connect = summary.errors.connect\n\
                   seen_percentile = latency:percentile(99.0)\n\
                   seen_length = #requests\n\
                   end\n",
    setup_transfer: "function setup(thread) thread:set(\"id\", 7) end\n",
    init_args: "function init(args) seen_first = args[0] end\n",
    full: "function request() return \"GET / HTTP/1.1\\r\\n\\r\\n\" end\n\
           function response(status, headers, body) end\n\
           function delay() return 0 end\n\
           function done(summary, latency, requests) end\n",
    runtime_error: "function request() error(\"boom\") end\n",
    syntax_error: "this is not lua\n",
    transfer_reject: "function setup(thread) thread:set(\"f\", print) end\n",
};

#[test]
fn lua_engine_conforms() {
    run("lua", super::factory, &SCRIPTS);
}
