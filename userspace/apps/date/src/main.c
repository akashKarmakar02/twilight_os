
#define _POSIX_C_SOURCE 200809L
#include <time.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <stdlib.h>

int main(void) {
    time_t t;
    if (time(&t) == (time_t)-1) return 1;

    setenv("TZ", ":/etc/localtime", 1);
    tzset();

    struct tm tm;
    char tz_name[32] = "UTC";
    if (!localtime_r(&t, &tm)) {
        if (!gmtime_r(&t, &tm)) return 1;
    } else {
        if (strftime(tz_name, sizeof(tz_name), "%Z", &tm) == 0 || tz_name[0] == '\0') {
            char tz_off[8] = {0};
            if (strftime(tz_off, sizeof(tz_off), "%z", &tm) > 0 &&
                (tz_off[0] == '+' || tz_off[0] == '-') &&
                strlen(tz_off) >= 5) {
                snprintf(tz_name, sizeof(tz_name), "UTC%c%c%c:%c%c",
                         tz_off[0], tz_off[1], tz_off[2], tz_off[3], tz_off[4]);
            } else {
                strcpy(tz_name, "LOCAL");
            }
        }
    }

    static const char *days[] = {"Sun","Mon","Tue","Wed","Thu","Fri","Sat"};
    static const char *months[] = {"Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"};

    int hour24 = tm.tm_hour;
    const char *ampm = hour24 >= 12 ? "PM" : "AM";
    int hour12 = hour24 % 12;
    if (hour12 == 0) hour12 = 12;

    char out[128];
    int n = snprintf(out, sizeof(out), "%s %s %02d %02d:%02d:%02d %s %s %04d\n",
                     days[tm.tm_wday],
                     months[tm.tm_mon],
                     tm.tm_mday,
                     hour12, tm.tm_min, tm.tm_sec,
                     ampm,
                     tz_name,
                     tm.tm_year + 1900);

    if (n < 0) return 1;

    ssize_t w = write(1, out, (size_t)n);
    (void)w;
    return 0;
}
