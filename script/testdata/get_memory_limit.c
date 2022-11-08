#include "ckb_syscalls.h"

int main() {
  if (ckb_get_memory_limit() == 8) {
    return 0;
  }
  return 1;
}
