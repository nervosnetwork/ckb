#include "ckb_syscalls.h"

int main() {
  uint64_t length = 5;
  if (ckb_set_content((uint8_t *)"hello", &length) != 0) {
    return 1;
  }
  if (length != 0) {
    return 1;
  }
  return 0;
}
