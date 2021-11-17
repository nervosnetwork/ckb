#include "ckb_syscalls.h"

int current_cycles() {
    return syscall(2042, 0, 0, 0, 0, 0, 0);
}

int main() {
    int prev = current_cycles();
    int curr;
    for (int i=0; i<4096; i++) {
        curr = current_cycles();
        if (curr <= prev) {
            return -1;
        }
        prev = curr;
    }
    return CKB_SUCCESS;
}
