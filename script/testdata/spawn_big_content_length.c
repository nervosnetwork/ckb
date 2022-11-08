#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  uint64_t content_length = 0xffffffff;
  int success = ckb_spawn(8, 1, 3, 0, 0, NULL, NULL, NULL, &content_length);
  if (success != 5) {
    return 1;
  }
  return 0;
}
