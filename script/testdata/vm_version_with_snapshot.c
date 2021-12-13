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

int vm_version() {
    return syscall(2041, 0, 0, 0, 0, 0, 0);
}

int main() {
#ifdef DEBUG
    char message[2048];
#endif
    int ver;
    for (int i=0; i<4096; i++) {
        ver = vm_version();
        sprintf(message, "version = %d", ver); ckb_debug(message);
        if (i > 16) {
            try_pause();
        }
        if (ver != 1) {
            return -1;
        }
    }
    return CKB_SUCCESS;
}
