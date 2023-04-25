#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  uint64_t spawn_content_length = 0xffffffff;
  spawn_args_t spgs = {
      .memory_limit = 8,
      .exit_code = NULL,
      .content = NULL,
      .content_length = &spawn_content_length,
  };
  int success = ckb_spawn(1, 3, 0, 0, NULL, &spgs);
  if (success != 5) {
    return 1;
  }
  return 0;
}
