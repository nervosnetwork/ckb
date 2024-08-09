#include "ckb_syscalls.h"

int main() {
    int argc = 0;
    char *argv[] = {};
    syscall(2043, 0, 3, 0, 0, argc, argv);
    return -1;
}
