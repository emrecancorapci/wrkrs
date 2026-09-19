// example script that demonstrates use of thread.stop()

var counter = 1

function response() {
    if (counter === 100) {
        wrk.thread.stop()
    }
    counter = counter + 1
}
