#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

int fib(int n) {
    if (n < 2) {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

int main() {
    // Use invalid calculations to make the current cycles a larger value.
    if (fib(20) != 6765) {
        return 1;
    }

    int cycles = ckb_current_cycles();
    char buffer[16] = {0};
    sprintf_(buffer, "%d", cycles);
    const char *argv[] = {&buffer[0], 0};
    return simple_spawn_args(1, 1, argv);
}
