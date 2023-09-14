#include "ckb_syscalls.h"

int main() {
  syscall(2043, 2, 3, 0, 0, 0, NULL);
  return -1;
}
