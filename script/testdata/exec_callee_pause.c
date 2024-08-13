#include "ckb_syscalls.h"

void try_pause() {
    ckb_debug("try_pause");
    syscall(2178, 0, 0, 0, 0, 0, 0);
}

int main(int argc, char* argv[]) {
    try_pause();
    if (argc != 3) {
        return 1;
    }
    try_pause();
    if (argv[0][0] != 'a') {
        return 2;
    }
    try_pause();
    if (argv[1][0] != 'b') {
        return 3;
    }
    try_pause();
    if (argv[2][0] != 'c') {
        return 4;
    }
    try_pause();
    return 0;
}
