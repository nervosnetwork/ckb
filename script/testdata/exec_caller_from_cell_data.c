#include "ckb_syscalls.h"

int main() {
    int argc = 3;
    char *argv[] = {"a", "b", "c"};
    syscall(2043, 1, 3, 0, 0, argc, argv);
    return -1;
}
