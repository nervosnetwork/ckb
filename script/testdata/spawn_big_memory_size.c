#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() {
  int success = ckb_spawn(9, 1, 3, 0, 0, NULL, NULL, NULL, NULL);
  if (success != 6) {
    return 1;
  }
  return 0;
}
