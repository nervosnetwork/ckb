#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#include "ckb_exec.h"
#include "ckb_syscalls.h"

int main() {
  const char *argv[] = {"hello", "world"};

  int8_t spawn_exit_code = -1;
  uint8_t spawn_content[80] = {};
  uint64_t spawn_content_length = 80;
  spawn_args_t spgs = {
      .memory_limit = 8,
      .exit_code = &spawn_exit_code,
      .content = &spawn_content[0],
      .content_length = &spawn_content_length,
  };
  uint8_t hash[32] = {};
  uint32_t hash_len = 0;
  _exec_hex2bin(
      "1dc91c6a0d93ffba6d11bae8bc74d4cb89506e58203e7361434e77f24eb7b11f", hash,
      32, &hash_len);
  int success = ckb_spawn_cell(hash, 0, 0, 0, 2, argv, &spgs);
  if (success != 0) {
    return 1;
  }
  if (spawn_exit_code != 0) {
    return 1;
  }
  if (strlen((char *)spawn_content) != 10) {
    return 1;
  }
  if (strcmp((char *)spawn_content, "helloworld") != 0) {
    return 1;
  }
  return 0;
}
