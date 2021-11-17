#include "ckb_syscalls.h"

int main() {
    int argc = 3;
    char *argv[] = {"a", "b", "c"};
    int ret = syscall(2043, 1, 3, 0, 0xffffffffffffffff, argc, argv);
    if (ret != 0) {
      return ret;
    }
    return -1;
}
