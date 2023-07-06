#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "ckb_syscalls.h"

int main(int argc, char *argv[]) {
  int caller_cycles = atoi(argv[0]);
  // Callee's current cycles must > caller's current cycles.
  int callee_cycles = ckb_current_cycles();
  if (callee_cycles < caller_cycles + 100000) {
    return 1;
  }
  return 0;
}
