/* Fixture-only wall clock. Monotonic/window timers keep the real clock. */
#define _GNU_SOURCE
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
int clock_gettime(clockid_t clock, struct timespec *value) {
    if (clock == CLOCK_REALTIME) {
        value->tv_sec = 1800000000;
        value->tv_nsec = 0;
        return 0;
    }
    return syscall(SYS_clock_gettime, clock, value);
}
