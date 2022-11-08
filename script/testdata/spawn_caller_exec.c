#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main() { return ckb_spawn(8, 1, 3, 0, 0, NULL, NULL, NULL, NULL); }
