#include "ckb_syscalls.h"

int main() {
    if (syscall(2041, 0, 0, 0, 0, 0, 0) == 2) {
      return 0;
    }
    return 1;
}
