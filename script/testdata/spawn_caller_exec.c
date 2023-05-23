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
  return ckb_spawn(1, 3, 0, 0, NULL, &spgs);
}
