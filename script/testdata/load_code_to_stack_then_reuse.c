/*  Script Description:
 *  - Args:
 *    - A little endian unsigned 8 bits integer: `flag`.
 *    - A little endian unsigned 64 bits integer: `size`.
 *    - The `code_hash`(`data_hash`) of a shared library.
 */

#include "ckb_dlfcn.h"
#include "ckb_syscalls.h"
#include "blockchain.h"

#ifdef DEBUG
#include <stdio.h>
char message[2048];
#else
#define ckb_debug(...)
#define sprintf(...)
#endif

#define SCRIPT_SIZE 32768
#define BUFFER_SIZE 32768

#define bool2char(b) ((b) ? '+' : '-')

uint64_t read_u64_le (const uint8_t *src) {
    return *(const uint64_t *)src;
}

int try_load_code(bool if_load_code, uint8_t* code_hash) {
    uint8_t buf[BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));
    sprintf(message, "%c X in [%p, %p)", bool2char(if_load_code), buf, buf+BUFFER_SIZE); ckb_debug(message);
    if (if_load_code) {
        void *handle = NULL;
        uint64_t consumed_size = 0;
        uint8_t hash_type = 0;
        int ret = ckb_dlopen2(code_hash, hash_type, buf, BUFFER_SIZE, &handle, &consumed_size);
        if (ret != CKB_SUCCESS) {
            return -5;
        }
    }
    return CKB_SUCCESS;
}

volatile void try_write_stack(bool if_write_stack, int size) {
    volatile uint8_t buf[size] __attribute__((aligned(RISCV_PGSIZE)));
    sprintf(message, "%c W in [%p, %p)", bool2char(if_write_stack), buf, buf+size); ckb_debug(message);
    if (if_write_stack) {
        for (int i=0; i<size; i++) {
            *(buf+i-1) = i;
        }
    }
}


int main (int argc, char *argv[]) {
    int ret;
    uint64_t len = SCRIPT_SIZE;
    uint8_t script[SCRIPT_SIZE];

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

    if (bytes_seg.size != 1 + 8 + 32) {
        return -4;
    }

    uint8_t flag = bytes_seg.ptr[0];
    volatile uint64_t size = read_u64_le(bytes_seg.ptr+1);
    sprintf(message, "flag = %x, size = %ld", flag, size); ckb_debug(message);

    bool if_init_stack  = (flag & 0b0001) == 0b0001;
    bool if_load_code   = (flag & 0b0010) == 0b0010;
    bool if_write_stack = (flag & 0b0100) == 0b0100;

    if (if_init_stack) {
        ret = try_load_code(if_load_code, bytes_seg.ptr+9);
        if (ret != 0) { return ret; }
    }
    try_write_stack(if_write_stack, size);
    return CKB_SUCCESS;
}
