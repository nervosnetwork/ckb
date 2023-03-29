#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main_lt_content_length() {
  int8_t exit_code = -1;
  uint8_t content[10] = {};
  uint64_t content_length = 10;
  const char *argv[] = {"8", "8"};
  uint64_t success = 0;

  success =
      ckb_spawn(8, 1, 3, 0, 2, argv, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 0) {
    return 1;
  }
  if (content_length != 8) {
    return 1;
  }
  return 0;
}

int main_eq_content_length() {
  int8_t exit_code = -1;
  uint8_t content[10] = {};
  uint64_t content_length = 10;
  const char *argv[] = {"10", "10"};
  uint64_t success = 0;

  success =
      ckb_spawn(8, 1, 3, 0, 2, argv, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 0) {
    return 1;
  }
  if (content_length != 10) {
    return 1;
  }
  return 0;
}

int main_gt_content_length() {
  int8_t exit_code = -1;
  uint8_t content[10] = {};
  uint64_t content_length = 10;
  const char *argv[] = {"12", "10"};
  uint64_t success = 0;

  success =
      ckb_spawn(8, 1, 3, 0, 2, argv, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 0) {
    return 1;
  }
  if (content_length != 10) {
    return 1;
  }
  return 0;
}

int main() {
  if (main_lt_content_length() != 0) {
    return 1;
  }
  if (main_eq_content_length() != 0) {
    return 1;
  }
  if (main_gt_content_length() != 0) {
    return 1;
  }
  return 0;
}
