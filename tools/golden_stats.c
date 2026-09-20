// Golden vector generator for the stats port.
//
// Build:
//   gcc -O2 -Isrc -o golden_stats tools/golden_stats.c src/stats.c -lm
//
// The zcalloc and zfree shims keep zmalloc out of the build, the
// printed lines are the reference the Rust golden test embeds.

#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>

#include "stats.h"

void *zcalloc(size_t size) { return calloc(1, size); }

void zfree(void *ptr) { free(ptr); }

static void dump(const char *label, stats *s) {
    printf("%s count=%" PRIu64 "\n", label, s->count);
    printf("%s min=%" PRIu64 " max=%" PRIu64 "\n", label, s->min, s->max);
    long double mean = stats_mean(s);
    long double stdev = stats_stdev(s, mean);
    printf("%s mean=%.6Lf\n", label, mean);
    printf("%s stdev=%.6Lf\n", label, stdev);
    printf("%s within=%.6Lf\n", label, stats_within_stdev(s, mean, stdev, 1));
    long double ps[] = {0, 25, 50, 75, 90, 99, 99.9, 100};
    const char *names[] = {"0", "25", "50", "75", "90", "99", "99.9", "100"};
    for (size_t i = 0; i < sizeof(ps) / sizeof(ps[0]); i++) {
        printf("%s p%s=%" PRIu64 "\n", label, names[i], stats_percentile(s, ps[i]));
    }
    uint64_t occupied = stats_popcount(s);
    printf("%s popcount=%" PRIu64 "\n", label, occupied);
    for (uint64_t i = 0; i <= occupied; i++) {
        uint64_t count = 0;
        uint64_t value = stats_value_at(s, i, &count);
        printf("%s value_at(%" PRIu64 ")=%" PRIu64 ":%" PRIu64 "\n", label, i, value, count);
    }
}

int main(void) {
    stats *s = stats_alloc(200);
    uint64_t values[] = {1, 5, 5, 10, 13, 13, 13, 40, 67, 128, 200, 4, 4};
    int rejected = 0;
    for (size_t i = 0; i < sizeof(values) / sizeof(values[0]); i++) {
        if (!stats_record(s, values[i])) rejected++;
    }
    if (!stats_record(s, 201)) rejected++;
    printf("rejected=%d\n", rejected);
    dump("before", s);
    stats_correct(s, 10);
    dump("after", s);
    return 0;
}
