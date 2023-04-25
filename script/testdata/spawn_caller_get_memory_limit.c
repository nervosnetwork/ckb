#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  int8_t spawn_exit_code;
  uint8_t spawn_content[80] = {};
  uint64_t spawn_content_length = 80;
  spawn_args_t spgs = {
      .memory_limit = 0,
      .exit_code = &spawn_exit_code,
      .content = &spawn_content[0],
      .content_length = &spawn_content_length,
  };
  uint64_t success = 0;

  spgs.memory_limit = 3;
  success = ckb_spawn(1, 3, 0, 0, NULL, &spgs);
  if (success != 0) {
    return 1;
  }
  if (spawn_exit_code != 3) {
    return 1;
  }

  spgs.memory_limit = 7;
  success = ckb_spawn(1, 3, 0, 0, NULL, &spgs);
  if (success != 0) {
    return 1;
  }
  if (spawn_exit_code != 7) {
    return 1;
  }

  spgs.memory_limit = 8;
  success = ckb_spawn(1, 3, 0, 0, NULL, &spgs);
  if (success != 0) {
    return 1;
  }
  if (spawn_exit_code != 8) {
    return 1;
  }

  return 0;
}
