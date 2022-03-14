#include "ckb_syscalls.h"
#include <stdio.h>

int main() {
    char message[2048];
    sprintf(message, "debugger print utf-8 string");
    ckb_debug(message);

    return CKB_SUCCESS;
}
