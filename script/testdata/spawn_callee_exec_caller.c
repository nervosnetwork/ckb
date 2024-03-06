#include <stdlib.h>
#include "ckb_syscalls.h"

int main() {
    // syscall exec
    syscall(2043, 2, 3, 0, 0, 0, NULL);
    return -1;
}
