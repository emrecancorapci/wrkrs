// example script demonstrating HTTP pipelining

var req

function init(args) {
    req = wrk.format(null, "/?foo")
        + wrk.format(null, "/?bar")
        + wrk.format(null, "/?baz")
}

function request() {
    return req
}
