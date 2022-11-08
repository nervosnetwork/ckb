#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#include "ckb_exec.h"
#include "ckb_syscalls.h"

int main() {
  const char *argv[] = {"hello", "world"};
  int8_t exit_code = 255;
  uint8_t content[80] = {};
  uint64_t content_length = 80;
  uint8_t hash[32] = {};
  uint32_t hash_len = 0;
  _exec_hex2bin(
      "b27be1358859d2bedced95e5616941fbbc4e5d4d043ad813cfe37ccec767c303", hash,
      32, &hash_len);
  int success = ckb_spawn_cell(8, hash, 0, 0, 0, 2, argv, &exit_code,
                               &content[0], &content_length);
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
