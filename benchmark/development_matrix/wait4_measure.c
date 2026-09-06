#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifndef __linux__
#error "wait4_measure currently defines ru_maxrss units only for Linux"
#endif

static volatile sig_atomic_t alarm_seen = 0;
static volatile sig_atomic_t forwarded_signal = 0;

static void on_alarm(int signal_number) {
    (void)signal_number;
    alarm_seen = 1;
}

static void on_forwarded_signal(int signal_number) {
    forwarded_signal = signal_number;
}

static void usage(const char *program) {
    fprintf(stderr,
            "usage: %s --output FILE --timeout-seconds N --grace-seconds N -- "
            "COMMAND [ARG ...]\n",
            program);
}

static bool parse_positive_uint(const char *text, unsigned int *output) {
    char *end = NULL;
    unsigned long value;

    if (text == NULL || text[0] == '\0' || text[0] == '-') {
        return false;
    }
    errno = 0;
    value = strtoul(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value == 0 ||
        value > UINT32_MAX) {
        return false;
    }
    *output = (unsigned int)value;
    return true;
}

static uint64_t elapsed_nanoseconds(const struct timespec *start,
                                    const struct timespec *end) {
    uint64_t seconds = (uint64_t)(end->tv_sec - start->tv_sec);
    int64_t nanoseconds = end->tv_nsec - start->tv_nsec;

    if (nanoseconds < 0) {
        seconds -= 1;
        nanoseconds += 1000000000L;
    }
    return seconds * UINT64_C(1000000000) + (uint64_t)nanoseconds;
}

static uint64_t timeval_microseconds(const struct timeval *value) {
    return (uint64_t)value->tv_sec * UINT64_C(1000000) +
           (uint64_t)value->tv_usec;
}

static int sleep_milliseconds(long milliseconds) {
    struct timespec remaining = {
        .tv_sec = milliseconds / 1000,
        .tv_nsec = (milliseconds % 1000) * 1000000L,
    };

    while (nanosleep(&remaining, &remaining) != 0) {
        if (errno != EINTR) {
            return -1;
        }
    }
    return 0;
}

static int install_handler(int signal_number, void (*handler)(int)) {
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = handler;
    sigemptyset(&action.sa_mask);
    return sigaction(signal_number, &action, NULL);
}

static int open_metrics(const char *path) {
    int descriptor = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC |
                                    O_NOFOLLOW,
                          S_IRUSR | S_IWUSR);
    if (descriptor < 0) {
        fprintf(stderr, "wait4_measure: cannot reserve metrics file %s: %s\n",
                path, strerror(errno));
    }
    return descriptor;
}

static int write_metrics(int descriptor, const char *path,
                         uint64_t wall_nanoseconds,
                         const struct rusage *usage_record, int wait_status,
                         bool timed_out, int interruption_signal) {
    const char *exit_kind = "unknown";
    int exit_code = -1;
    int term_signal = 0;
    if (WIFEXITED(wait_status)) {
        exit_kind = "exited";
        exit_code = WEXITSTATUS(wait_status);
    } else if (WIFSIGNALED(wait_status)) {
        exit_kind = "signaled";
        term_signal = WTERMSIG(wait_status);
    }

    char exit_code_json[32];
    char term_signal_json[32];
    char interruption_json[32];
    if (exit_code >= 0) {
        (void)snprintf(exit_code_json, sizeof(exit_code_json), "\"%d\"",
                       exit_code);
    } else {
        (void)snprintf(exit_code_json, sizeof(exit_code_json), "null");
    }
    if (term_signal > 0) {
        (void)snprintf(term_signal_json, sizeof(term_signal_json), "\"%d\"",
                       term_signal);
    } else {
        (void)snprintf(term_signal_json, sizeof(term_signal_json), "null");
    }
    if (interruption_signal > 0) {
        (void)snprintf(interruption_json, sizeof(interruption_json), "\"%d\"",
                       interruption_signal);
    } else {
        (void)snprintf(interruption_json, sizeof(interruption_json), "null");
    }

    int written = dprintf(
        descriptor,
        "{\n"
        "  \"schema_version\": \"veritasm-linux-wait4-measure-v1\",\n"
        "  \"clock\": \"CLOCK_MONOTONIC\",\n"
        "  \"resource_api\": \"wait4_rusage_direct_child\",\n"
        "  \"max_rss_unit\": \"kibibytes_linux_ru_maxrss\",\n"
        "  \"wall_nanoseconds_decimal\": \"%" PRIu64 "\",\n"
        "  \"user_microseconds_decimal\": \"%" PRIu64 "\",\n"
        "  \"system_microseconds_decimal\": \"%" PRIu64 "\",\n"
        "  \"max_rss_kib_decimal\": \"%ld\",\n"
        "  \"exit_kind\": \"%s\",\n"
        "  \"exit_code_decimal\": %s,\n"
        "  \"termination_signal_decimal\": %s,\n"
        "  \"timed_out\": %s,\n"
        "  \"wrapper_interruption_signal_decimal\": %s\n"
        "}\n",
        wall_nanoseconds, timeval_microseconds(&usage_record->ru_utime),
        timeval_microseconds(&usage_record->ru_stime), usage_record->ru_maxrss,
        exit_kind, exit_code_json, term_signal_json,
        timed_out ? "true" : "false", interruption_json);
    if (written < 0) {
        fprintf(stderr, "wait4_measure: cannot write metrics file %s: %s\n",
                path, strerror(errno));
        close(descriptor);
        return -1;
    }

    int sync_status = fsync(descriptor);
    int sync_errno = errno;
    int close_status = close(descriptor);
    int close_errno = errno;
    if (sync_status != 0 || close_status != 0) {
        errno = sync_status != 0 ? sync_errno : close_errno;
        fprintf(stderr, "wait4_measure: cannot sync metrics file %s: %s\n",
                path, strerror(errno));
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    const char *output_path = NULL;
    unsigned int timeout_seconds = 0;
    unsigned int grace_seconds = 0;
    int command_index = 0;

    for (int index = 1; index < argc; index++) {
        if (strcmp(argv[index], "--") == 0) {
            command_index = index + 1;
            break;
        }
        if (strcmp(argv[index], "--output") == 0 && index + 1 < argc) {
            output_path = argv[++index];
        } else if (strcmp(argv[index], "--timeout-seconds") == 0 &&
                   index + 1 < argc) {
            if (!parse_positive_uint(argv[++index], &timeout_seconds)) {
                usage(argv[0]);
                return 125;
            }
        } else if (strcmp(argv[index], "--grace-seconds") == 0 &&
                   index + 1 < argc) {
            if (!parse_positive_uint(argv[++index], &grace_seconds)) {
                usage(argv[0]);
                return 125;
            }
        } else {
            usage(argv[0]);
            return 125;
        }
    }
    if (output_path == NULL || timeout_seconds == 0 || grace_seconds == 0 ||
        command_index == 0 || command_index >= argc) {
        usage(argv[0]);
        return 125;
    }
    if (timeout_seconds > 86400 || grace_seconds > 600) {
        fprintf(stderr,
                "wait4_measure: timeout must be <=86400 seconds and grace must "
                "be <=600 seconds\n");
        return 125;
    }

    if (install_handler(SIGALRM, on_alarm) != 0 ||
        install_handler(SIGINT, on_forwarded_signal) != 0 ||
        install_handler(SIGTERM, on_forwarded_signal) != 0 ||
        install_handler(SIGHUP, on_forwarded_signal) != 0) {
        fprintf(stderr, "wait4_measure: cannot install signal handlers: %s\n",
                strerror(errno));
        return 125;
    }

    int metrics_descriptor = open_metrics(output_path);
    if (metrics_descriptor < 0) {
        return 125;
    }

    struct timespec start;
    if (clock_gettime(CLOCK_MONOTONIC, &start) != 0) {
        fprintf(stderr, "wait4_measure: clock_gettime failed: %s\n",
                strerror(errno));
        close(metrics_descriptor);
        return 125;
    }

    pid_t child = fork();
    if (child < 0) {
        fprintf(stderr, "wait4_measure: fork failed: %s\n", strerror(errno));
        close(metrics_descriptor);
        return 125;
    }
    if (child == 0) {
        (void)setpgid(0, 0);
        execvp(argv[command_index], &argv[command_index]);
        fprintf(stderr, "wait4_measure: exec failed for %s: %s\n",
                argv[command_index], strerror(errno));
        _exit(127);
    }
    if (setpgid(child, child) != 0 && errno != EACCES && errno != ESRCH) {
        fprintf(stderr, "wait4_measure: setpgid failed: %s\n", strerror(errno));
        (void)kill(child, SIGKILL);
        (void)waitpid(child, NULL, 0);
        close(metrics_descriptor);
        return 125;
    }

    alarm(timeout_seconds);
    int wait_status = 0;
    struct rusage usage_record;
    memset(&usage_record, 0, sizeof(usage_record));
    bool timeout_action = false;
    bool forward_action = false;

    for (;;) {
        pid_t waited = wait4(child, &wait_status, 0, &usage_record);
        if (waited == child) {
            break;
        }
        if (waited < 0 && errno == EINTR) {
            if (alarm_seen != 0) {
                timeout_action = true;
                break;
            }
            if (forwarded_signal != 0) {
                forward_action = true;
                break;
            }
            continue;
        }
        fprintf(stderr, "wait4_measure: wait4 failed: %s\n", strerror(errno));
        (void)kill(-child, SIGKILL);
        (void)wait4(child, &wait_status, 0, &usage_record);
        close(metrics_descriptor);
        return 125;
    }
    alarm(0);

    if (timeout_action || forward_action) {
        int signal_number = timeout_action ? SIGTERM : forwarded_signal;
        (void)kill(-child, signal_number);
        unsigned int polls = grace_seconds * 100;
        bool reaped = false;
        for (unsigned int poll = 0; poll < polls; poll++) {
            pid_t waited = wait4(child, &wait_status, WNOHANG, &usage_record);
            if (waited == child) {
                reaped = true;
                break;
            }
            if (waited < 0 && errno != EINTR) {
                break;
            }
            if (sleep_milliseconds(10) != 0) {
                break;
            }
        }
        if (!reaped) {
            (void)kill(-child, SIGKILL);
            while (wait4(child, &wait_status, 0, &usage_record) < 0 &&
                   errno == EINTR) {
            }
        }
    }

    struct timespec end;
    if (clock_gettime(CLOCK_MONOTONIC, &end) != 0) {
        fprintf(stderr, "wait4_measure: clock_gettime failed: %s\n",
                strerror(errno));
        close(metrics_descriptor);
        return 125;
    }
    if (write_metrics(metrics_descriptor, output_path,
                      elapsed_nanoseconds(&start, &end),
                      &usage_record, wait_status, timeout_action,
                      forward_action ? forwarded_signal : 0) != 0) {
        return 125;
    }
    if (timeout_action) {
        return 124;
    }
    if (forward_action) {
        return 128 + forwarded_signal;
    }
    if (WIFEXITED(wait_status)) {
        return WEXITSTATUS(wait_status);
    }
    if (WIFSIGNALED(wait_status)) {
        return 128 + WTERMSIG(wait_status);
    }
    return 125;
}
