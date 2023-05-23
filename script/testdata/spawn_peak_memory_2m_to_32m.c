#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "ckb_syscalls.h"

int main(int argc, char *argv[]) {
  int8_t spawn_exit_code = 255;
  spawn_args_t spgs = {
      .memory_limit = 4,
      .exit_code = &spawn_exit_code,
      .content = NULL,
      .content_length = NULL,
  };
  int8_t can_i_spawn = 0;
  if (argc == 0) {
    can_i_spawn = 1;
  }
  uint64_t depth = (uint64_t)atoi(argv[0]);
  if (depth < 14) {
    can_i_spawn = 1;
  }
  if (can_i_spawn) {
    char buffer[20];
    itoa(depth + 1, buffer, 10);
    const char *argv[] = {buffer};
    uint64_t success = ckb_spawn(0, 3, 0, 1, argv, &spgs);
    if (success != 0) {
      return success;
    }
    if (spawn_exit_code != 0) {
      return 1;
    }
  }
  return 0;
}
