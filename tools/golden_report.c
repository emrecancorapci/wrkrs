// Golden vector generator for the legacy report port.
//
// Build:
//   gcc -O2 -Isrc -Iobj/include -Iobj/include/luajit-2.1 \
//       -o golden_report tools/golden_report.c \
//       obj/net.o obj/ssl.o obj/aprintf.o obj/stats.o obj/script.o \
//       obj/units.o obj/ae.o obj/zmalloc.o obj/http_parser.o \
//       obj/bytecode.o obj/version.o \
//       -Lobj/lib -Wl,-E -lluajit-5.1 -lssl -lcrypto -lm -lpthread -ldl
//
// The print functions come from wrk.c itself, included below with
// its main renamed, so the fixture bytes are the real C output. The
// footer printfs live inline in the C main and are replicated here
// line for line.

#define main wrk_original_main
#include "wrk.c"
#undef main

static stats *build(uint64_t max, const uint64_t *values, size_t count) {
    stats *s = stats_alloc(max);
    for (size_t i = 0; i < count; i++) stats_record(s, values[i]);
    return s;
}

int main(void) {
    char *time = format_time_s(30);
    printf("Running %s test @ %s\n", time, "http://127.0.0.1:8080/");
    printf("  %"PRIu64" threads and %"PRIu64" connections\n",
           (uint64_t) 2, (uint64_t) 10);

    uint64_t lvals[] = {
        34, 53, 77, 171, 500, 1190, 1200, 1500, 1500, 2000, 3400, 171000
    };
    stats *latency = build(2000000, lvals, sizeof(lvals) / sizeof(lvals[0]));

    uint64_t rvals[] = { 95400, 98700, 99800, 100100, 100300, 105900, 107200 };
    stats *rate = build(10000000, rvals, sizeof(rvals) / sizeof(rvals[0]));

    print_stats_header();
    print_stats("Latency", latency, format_time_us);
    print_stats("Req/Sec", rate, format_metric);
    print_stats_latency(latency);

    uint64_t runtime_us = 2123456;
    uint64_t complete   = 424167;
    uint64_t bytes      = 16800000;

    long double runtime_s   = runtime_us / 1000000.0;
    long double req_per_s   = complete   / runtime_s;
    long double bytes_per_s = bytes      / runtime_s;

    char *runtime_msg = format_time_us(runtime_us);

    printf("  %"PRIu64" requests in %s, %sB read\n",
           complete, runtime_msg, format_binary(bytes));
    printf("Requests/sec: %9.2Lf\n", req_per_s);
    printf("Transfer/sec: %10sB\n", format_binary(bytes_per_s));

    return 0;
}
