#include <stdint.h>
#include <stdlib.h>

#include "ckb_syscalls.h"

int main(int argc, char *argv[]) {
  uint64_t size = (uint64_t)atoi(argv[0]);
  uint64_t real = (uint64_t)atoi(argv[1]);
  uint8_t data[20] = {};
  int success = ckb_set_content(&data[0], &size);
  if (success != 0) {
    return 1;
  }
  if (size != real) {
    return 1;
  }
  return 0;
}
