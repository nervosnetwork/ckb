#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  int8_t exit_code = 255;
  uint8_t content[80] = {};
  uint64_t content_length = 80;
  uint64_t success = 0;

  success =
      ckb_spawn(3, 1, 3, 0, 0, NULL, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 3) {
    return 1;
  }

  success =
      ckb_spawn(7, 1, 3, 0, 0, NULL, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 7) {
    return 1;
  }

  success =
      ckb_spawn(8, 1, 3, 0, 0, NULL, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 8) {
    return 1;
  }

  return 0;
}
