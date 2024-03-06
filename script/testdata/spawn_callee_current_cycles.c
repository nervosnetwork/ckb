#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "ckb_syscalls.h"

int atoi(const char *s) {
    int n = 0, neg = 0;
    switch (*s) {
        case '-':
            neg = 1;
        case '+':
            s++;
    }
    /* Compute n as a negative number to avoid overflow on INT_MIN */
    while (_is_digit(*s)) n = 10 * n - (*s++ - '0');
    return neg ? n : -n;
}

int main(int argc, const char *argv[]) {
    int caller_cycles = atoi(argv[0]);
    // Callee's current cycles must > caller's current cycles.
    int callee_cycles = ckb_current_cycles();
    if (callee_cycles < caller_cycles + 100000) {
        return 1;
    }
    return 0;
}
