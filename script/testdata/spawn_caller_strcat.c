#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  const char *argv[] = {"hello", "world"};
  int8_t exit_code = 255;
  uint8_t content[80] = {};
  uint64_t content_length = 80;

  int success =
      ckb_spawn(8, 1, 3, 0, 2, argv, &exit_code, &content[0], &content_length);
  if (success != 0) {
    return 1;
  }
  if (exit_code != 0) {
    return 1;
  }
  if (strlen((char *)content) != 10) {
    return 1;
  }
  if (strcmp((char *)content, "helloworld") != 0) {
    return 1;
  }
  return 0;
}
