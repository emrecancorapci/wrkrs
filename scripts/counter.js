// example dynamic request script which demonstrates changing
// the request path and a header for each request
//
// NOTE: each wrk thread has an independent scripting context
// and thus there will be one counter per thread

var counter = 0

function request() {
    var path = "/" + counter
    wrk.headers["X-Counter"] = counter
    counter = counter + 1
    return wrk.format(null, path)
}
