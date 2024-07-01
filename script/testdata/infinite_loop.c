#include "ckb_syscalls.h"

#ifdef DEBUG
#include <stdio.h>
#else
#define ckb_debug(...)
#define sprintf(...)
#endif


int main() {
    for(; ;) {}
    return CKB_SUCCESS;
}
