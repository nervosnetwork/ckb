#include "ckb_dlfcn.h"
#include "ckb_syscalls.h"
#include "protocol.h"

#ifndef DEBUG
#include <stdio.h>

#define ckb_debug(...)
#define sprintf(...)
#endif

#define SCRIPT_SIZE 32768

void try_pause() {
    syscall(2178, 0, 0, 0, 0, 0, 0);
}

uint64_t read_u64_le (const uint8_t *src) {
    return *(const uint64_t *)src;
}

int main (int argc, char *argv[]) {
    int ret;
    uint64_t len = SCRIPT_SIZE;
    uint8_t script[SCRIPT_SIZE];
#ifdef DEBUG
    char message[2048];
#endif

    ret = ckb_load_script(script, &len, 0);
    if (ret != CKB_SUCCESS) {
        return -1;
    }
    if (len > SCRIPT_SIZE) {
        return -2;
    }

    mol_seg_t script_seg;
    mol_seg_t args_seg;
    mol_seg_t bytes_seg;
    script_seg.ptr = (uint8_t *)script;
    script_seg.size = len;
    if (MolReader_Script_verify(&script_seg, false) != MOL_OK) {
        return -3;
    }
    args_seg = MolReader_Script_get_args(&script_seg);
    bytes_seg = MolReader_Bytes_raw_bytes(&args_seg);

    if (bytes_seg.size != 8 + 32) {
        return -4;
    }

    volatile uint64_t number = read_u64_le(bytes_seg.ptr);
    sprintf(message, "number = %ld", number); ckb_debug(message);

    if (number == 0) {
        return CKB_SUCCESS;
    }

    bool is_even = false;
    {
        void *handle = NULL;
        uint64_t consumed_size = 0;

        uint64_t code_buffer_size = 100 * 1024;
        uint8_t code_buffer[code_buffer_size] __attribute__((aligned(RISCV_PGSIZE)));
        uint8_t hash_type = 0;
        ret = ckb_dlopen2(bytes_seg.ptr+8, hash_type, code_buffer, code_buffer_size, &handle, &consumed_size);
        if (ret != CKB_SUCCESS) {
            return ret;
        }
        bool (*func)(int);
        *(void **)(&func) = ckb_dlsym(handle, "is_even");
        if (func == NULL) {
            return -6;
        }
        try_pause();
        is_even = func(number);
    }

    sprintf(message, "is_even(%ld) = %d", number, is_even); ckb_debug(message);

    if (is_even) {
        return -8;
    }

    return CKB_SUCCESS;
}
