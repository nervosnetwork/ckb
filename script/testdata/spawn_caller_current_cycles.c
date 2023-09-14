#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "ckb_syscalls.h"

int fib(int n) {
  if (n < 2) {
    return n;
  }
  return fib(n - 1) + fib(n - 2);
}

int main() {
  // Use invalid calculations to make the current cycles a larger value.
  if (fib(20) != 6765) {
    return 1;
  }

  int cycles = ckb_current_cycles();
  char buffer[8];
  itoa(cycles, buffer, 10);
  const char *argv[] = { &buffer[0] };
  int8_t spawn_exit_code = 255;
  uint8_t spawn_content[80] = {};
  uint64_t spawn_content_length = 80;
  spawn_args_t spgs = {
      .memory_limit = 8,
      .exit_code = &spawn_exit_code,
      .content = &spawn_content[0],
      .content_length = &spawn_content_length,
  };
  int success = ckb_spawn(1, 3, 0, 1, argv, &spgs);
  if (success != 0) {
    return 1;
  }
  if (spawn_exit_code != 0) {
    return 1;
  }
  return 0;
}
