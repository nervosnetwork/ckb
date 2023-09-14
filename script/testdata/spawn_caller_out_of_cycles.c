#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  spawn_args_t spgs = {
      .memory_limit = 8,
      .exit_code = NULL,
      .content = NULL,
      .content_length = NULL,
  };
  uint64_t success = ckb_spawn(1, 3, 0, 0, NULL, &spgs);
  if (success == 0) {
    return 1;
  }
  return 0;
}
