
#define _POSIX_C_SOURCE 200809L
#include <time.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <stdlib.h>

int main(void) {
    time_t t;
    if (time(&t) == (time_t)-1) return 1;

    t += 19800;

    struct tm tm;
    if (!gmtime_r(&t, &tm)) return 1;

    static const char *days[] = {"Sun","Mon","Tue","Wed","Thu","Fri","Sat"};
    static const char *months[] = {"Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"};

    int hour24 = tm.tm_hour;
    const char *ampm = hour24 >= 12 ? "PM" : "AM";
    int hour12 = hour24 % 12;
    if (hour12 == 0) hour12 = 12;

    char out[128];
    int n = snprintf(out, sizeof(out), "%s %s %02d %02d:%02d:%02d %s IST %04d\n",
                     days[tm.tm_wday],
                     months[tm.tm_mon],
                     tm.tm_mday,
                     hour12, tm.tm_min, tm.tm_sec,
                     ampm,
                     tm.tm_year + 1900);

    if (n < 0) return 1;

    ssize_t w = write(1, out, (size_t)n);
    (void)w;
    return 0;
}