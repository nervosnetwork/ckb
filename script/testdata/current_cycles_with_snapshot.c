#include "ckb_syscalls.h"

#ifdef DEBUG
#include <stdio.h>
#else
#define ckb_debug(...)
#define sprintf(...)
#endif

void try_pause() {
    syscall(2178, 0, 0, 0, 0, 0, 0);
}

int current_cycles() {
    return syscall(2042, 0, 0, 0, 0, 0, 0);
}

int main() {
#ifdef DEBUG
    char message[2048];
#endif
    int prev = current_cycles();
    int curr;
    for (int i=0; i<4096; i++) {
        curr = current_cycles();
        sprintf(message, "prev = %d, curr = %d", prev, curr); ckb_debug(message);
        if (i > 16) {
            try_pause();
        }
        if (curr <= prev) {
            return -1;
        }
        prev = curr;
    }
    return CKB_SUCCESS;
}
