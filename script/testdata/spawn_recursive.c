#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  int8_t exit_code = 255;
  uint64_t success = ckb_spawn(8, 0, 3, 0, 0, NULL, &exit_code, NULL, NULL);
  if (success != 0) {
    return success;
  }
  return exit_code;
}
