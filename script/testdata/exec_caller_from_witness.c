#include "ckb_syscalls.h"

int main() {
    int argc = 3;
    char *argv[] = {"a", "b", "c"};
    syscall(2043, 0, 1, 1, 0, argc, argv);
    return -1;
}
