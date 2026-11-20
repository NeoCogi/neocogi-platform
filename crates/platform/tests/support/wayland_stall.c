/* Pipe pacing for the independently spawned SDL selection owner in the
 * private Wayland acceptance test. Other writes go straight to the kernel.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

ssize_t write(int fd, const void *buffer, size_t length) {
    static const char selection[] = "External Wayland selection";
    const char *directory = getenv("NEOCOGI_TEST_ISOLATED_WAYLAND_DIRECTORY");
    if (directory != NULL && length >= sizeof(selection) - 1 &&
        memcmp(buffer, selection, sizeof(selection) - 1) == 0) {
        /* The peer owns a dedicated temporary working directory supplied by Rust. */
        const int after_prefix = access("stall-after-prefix", F_OK) == 0;
        if (after_prefix || access("stall-before-data", F_OK) == 0) {
            ssize_t count = 0;
            if (after_prefix) {
                count = syscall(SYS_write, fd, buffer, sizeof("External") - 1);
                if (count < 0)
                    return count;
            }
            struct timespec delay = { .tv_sec = 0, .tv_nsec = 150000000 };
            while (nanosleep(&delay, &delay) < 0 && errno == EINTR) {}
            if (after_prefix)
                return count;
        }
    }
    return syscall(SYS_write, fd, buffer, length);
}
