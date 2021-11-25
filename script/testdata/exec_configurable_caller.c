/*  Script Description:
 *  - Args:
 *    See "exec_configurable_callee.c".
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

#define EXEC_ARGC 9
#define SCRIPT_SIZE 32768
#define BUFFER_SIZE 32768

typedef uint64_t(arithmetic_func_t) (uint64_t);

uint8_t CODE_BUFFER[BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));

void to_hex(char* dst, uint8_t* src, size_t len) {
    for (size_t i = 0; i<len; i++) {
        char hi = src[i] >> 4;
        char lo = src[i] & 0xf;
        dst[i*2] = hi + (hi < 10 ? '0' : ('a' - 10));
        dst[i*2+1] = lo + (lo < 10 ? '0' : ('a' - 10));
    }
    dst[len*2] = '\0';
}

void write_u64_le_to_hex (char* dst, uint64_t number) {
    uint8_t* bytes = (uint8_t *)(&number);
    to_hex(dst, bytes, 8);
}

void try_pause() {
    syscall(2178, 0, 0, 0, 0, 0, 0);
}

uint64_t read_u64_le (const uint8_t *src) {
    return *(const uint64_t *)src;
}

int try_load_code(uint64_t* number, uint8_t* code_hash) {
    sprintf(message, "X in [%p, %p)", CODE_BUFFER, CODE_BUFFER+BUFFER_SIZE); ckb_debug(message);
    void *handle = NULL;
    uint64_t consumed_size = 0;
    uint8_t hash_type = 0;
    int ret = ckb_dlopen2(code_hash, hash_type, CODE_BUFFER, BUFFER_SIZE, &handle, &consumed_size);
    if (ret != CKB_SUCCESS) {
        return -6;
    }
    try_pause();
    arithmetic_func_t* func = (arithmetic_func_t*) ckb_dlsym(handle, "apply");
    if (func == NULL) {
        return -7;
    }
    try_pause();
    *number = func(*number);
    return CKB_SUCCESS;
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

    if (bytes_seg.size != 1 + 8*7 + 32) {
        return -4;
    }

    uint8_t flag       = bytes_seg.ptr[0];
    uint64_t recursion = read_u64_le(bytes_seg.ptr+1);
    uint64_t number    = read_u64_le(bytes_seg.ptr+1+8);
    uint64_t expected  = read_u64_le(bytes_seg.ptr+1+8*2);
    uint64_t index     = read_u64_le(bytes_seg.ptr+1+8*3);
    uint64_t source    = read_u64_le(bytes_seg.ptr+1+8*4);
    uint64_t place     = read_u64_le(bytes_seg.ptr+1+8*5);
    uint64_t bounds    = read_u64_le(bytes_seg.ptr+1+8*6);

    sprintf(message, "flag      = %x",  flag     ); ckb_debug(message);
    sprintf(message, "recursion = %ld", recursion); ckb_debug(message);
    sprintf(message, "number    = %ld", number   ); ckb_debug(message);
    sprintf(message, "expected  = %ld", expected ); ckb_debug(message);
    sprintf(message, "index     = %ld", index    ); ckb_debug(message);
    sprintf(message, "source    = %ld", source   ); ckb_debug(message);
    sprintf(message, "place     = %ld", place    ); ckb_debug(message);
    sprintf(message, "bounds    = %ld", bounds   ); ckb_debug(message);

    try_pause();

    if (recursion == 0) {
        if (number == expected) {
            return CKB_SUCCESS;
        } else {
            return -5;
        }
    }

    bool if_load_before_exec = (flag & 0b0001) == 0b0001;
    if (if_load_before_exec) {
        ret = try_load_code(&number, bytes_seg.ptr+1+8*7);
        if (ret != CKB_SUCCESS) {
            return ret;
        }
        try_pause();
        sprintf(message, "(apply before exec) number = %ld", number); ckb_debug(message);
    }

    {
        int argc_new = EXEC_ARGC;
        char flag_str[1*2+1];
        char recursion_str[8*2+1];
        char number_str[8*2+1];
        char expected_str[8*2+1];
        char index_str[8*2+1];
        char source_str[8*2+1];
        char place_str[8*2+1];
        char bounds_str[8*2+1];
        char code_hash_str[32*2+1];
        char *argv_new[EXEC_ARGC] = {
            flag_str, recursion_str, number_str, expected_str,
            index_str, source_str, place_str, bounds_str, code_hash_str };
        to_hex(flag_str,      bytes_seg.ptr, 1);
        write_u64_le_to_hex(recursion_str, recursion-1);
        write_u64_le_to_hex(number_str, number-1);
        write_u64_le_to_hex(expected_str, expected);
        to_hex(index_str,     bytes_seg.ptr+1+8*3, 8);
        to_hex(source_str,    bytes_seg.ptr+1+8*4, 8);
        to_hex(place_str,     bytes_seg.ptr+1+8*5, 8);
        to_hex(bounds_str,    bytes_seg.ptr+1+8*6, 8);
        to_hex(code_hash_str, bytes_seg.ptr+1+8*7, 32);
        syscall(2043, index, source, place, bounds, argc_new, argv_new);
    }

    try_pause();

    return CKB_SUCCESS;
}
