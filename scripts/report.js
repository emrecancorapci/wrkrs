// example reporting script which demonstrates a custom
// done function that prints latency percentiles as CSV

function done(summary, latency, requests) {
    print("------------------------------\n")
    for (var i = 0; i < 4; i++) {
        var p = [50, 90, 99, 99.999][i]
        print(p + "%," + latency.percentile(p) + "\n")
    }
}
