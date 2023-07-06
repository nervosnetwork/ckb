#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main(int argc, char *argv[]) {
  int peak_memory = ckb_peak_memory();
  if (peak_memory != (argc + 1) * 8) {
    return 1;
  }
  if (peak_memory < 56) {
    int spawn_argc = argc + 1;
    const char *spawn_argv[] = {"", "", "", "", "", "", "", ""};
    int8_t spawn_exit_code = 255;
    spawn_args_t spgs = {
        .memory_limit = 8,
        .exit_code = &spawn_exit_code,
        .content = NULL,
        .content_length = NULL,
    };
    uint64_t success = ckb_spawn(0, 3, 0, spawn_argc, spawn_argv, &spgs);
    if (success != 0) {
      return success;
    }
  } else {
    return 0;
  }
}
