/*  Script Description:
 *  - Args:
 *    - Arguments for the result.
 *      - A 8 bits flag.
 *        - 1st lowest bit: if apply the function in loaded code to the `number`
 *          before recursively exec.
 *        - 2st lowest bit: if write the stack.
 *        - 3st lowest bit: if apply the function in loaded code to the `number`
 *          after recursively exec.
 *      - A hex string of a little endian unsigned 64 bits integer: `recursion`.
 *        - Recursively call `exec` how many times.
 *          If `recursion > 0`, call `recursion--` and calculate a new `number`,
 *          then exec with all arguments again.
 *        - If `recursion == 0`, then stop the recursively exec.
 *      - A hex string of a little endian unsigned 64 bits integer: `number`.
 *      - A hex string of a little endian unsigned 64 bits integer: `expected`.
 *        Returns 0 if the last `number` is equal to `expected`, otherwise
 *        returns 1.
 *    - Arguments for `exec`.
 *      - A hex string of a little endian unsigned 64 bits integer: `index`.
 *      - A hex string of a little endian unsigned 64 bits integer: `source`.
 *      - A hex string of a little endian unsigned 64 bits integer: `place`.
 *      - A hex string of a little endian unsigned 64 bits integer: `bounds`.
 *    - Arguments for `load_data_as_code`.
 *      - The `code_hash`(`data_hash`) of a shared library.
 */

#include "stdbool.h"
#include "ckb_dlfcn.h"
#include "ckb_syscalls.h"

#ifdef DEBUG
#include <stdio.h>
char message[2048];
#else
#define ckb_debug(...)
#define sprintf(...)
#endif

#define EXEC_ARGC 9
#define BUFFER_SIZE 32768

typedef uint64_t(arithmetic_func_t) (uint64_t);

uint8_t CODE_BUFFER[BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));

void try_pause() {
    syscall(2178, 0, 0, 0, 0, 0, 0);
}

void from_hex (uint8_t* dst, char* src, size_t len) {
    for (size_t i=0; i<len; i++) {
        uint8_t hi = src[i*2];
        uint8_t lo = src[i*2+1];
        dst[i] = (((hi & 0xf) + (hi >> 6) * 9) << 4) | (((lo & 0xf) + (lo >> 6) * 9));
    }
}

void to_hex(char* dst, uint8_t* src, size_t len) {
    for (size_t i = 0; i<len; i++) {
        char hi = src[i] >> 4;
        char lo = src[i] & 0xf;
        dst[i*2] = hi + (hi < 10 ? '0' : ('a' - 10));
        dst[i*2+1] = lo + (lo < 10 ? '0' : ('a' - 10));
    }
    dst[len*2] = '\0';
}

volatile uint64_t read_u64_le_from_hex (char* src) {
    uint8_t bytes[8];
    from_hex(bytes, src, 8);
    return *(const uint64_t *)bytes;
}

void write_u64_le_to_hex (char* dst, uint64_t number) {
    uint8_t* bytes = (uint8_t *)(&number);
    to_hex(dst, bytes, 8);
}

int try_exec(char*argv[], uint64_t recursion, uint64_t number) {
    sprintf(message, "argv[4] = %s", argv[4]); ckb_debug(message);
    if (strlen(argv[4]) != 8*2) {
        return -21;
    }
    uint64_t index = read_u64_le_from_hex(argv[4]);

    sprintf(message, "argv[5] = %s", argv[5]); ckb_debug(message);
    if (strlen(argv[5]) != 8*2) {
        return -22;
    }
    uint64_t source = read_u64_le_from_hex(argv[5]);

    sprintf(message, "argv[6] = %s", argv[6]); ckb_debug(message);
    if (strlen(argv[6]) != 8*2) {
        return -23;
    }
    uint64_t place = read_u64_le_from_hex(argv[6]);

    sprintf(message, "argv[7] = %s", argv[7]); ckb_debug(message);
    if (strlen(argv[7]) != 8*2) {
        return -24;
    }
    uint64_t bounds = read_u64_le_from_hex(argv[7]);

    char recursion_str[8*2+1];
    char number_str[8*2+1];
    char *argv_new[EXEC_ARGC] = {
        argv[0], recursion_str, number_str, argv[3],
        argv[4], argv[5], argv[6], argv[7], argv[8] };
    write_u64_le_to_hex(recursion_str, recursion);
    write_u64_le_to_hex(number_str, number);
    try_pause();
    syscall(2043, index, source, place, bounds, EXEC_ARGC, argv_new);
    return CKB_SUCCESS;
}

int try_load_code(uint64_t* number, uint8_t* code_hash) {
    void *handle = NULL;
    uint64_t consumed_size = 0;
    uint8_t hash_type = 0;
    int ret = ckb_dlopen2(code_hash, hash_type, CODE_BUFFER, BUFFER_SIZE, &handle, &consumed_size);
    if (ret != CKB_SUCCESS) {
        return -31;
    }
    try_pause();
    arithmetic_func_t* func = (arithmetic_func_t*) ckb_dlsym(handle, "apply");
    if (func == NULL) {
        return -32;
    }
    try_pause();
    *number = func(*number);
    return CKB_SUCCESS;
}

int main (int argc, char *argv[]) {
    sprintf(message, "argc = %d", argc); ckb_debug(message);
    if (argc != EXEC_ARGC) {
        return -11;
    }

    sprintf(message, "argv[0] = %s", argv[0]); ckb_debug(message);
    if (strlen(argv[0]) != 1*2) {
        return -12;
    }
    uint8_t flag;
    from_hex(&flag, argv[0], 1);

    bool if_write_stack      = (flag & 0b0010) == 0b0010;
    if (if_write_stack) {
        sprintf(message, "(try update the stack)"); ckb_debug(message);
        sprintf(message, "W in [%p, %p)", CODE_BUFFER, CODE_BUFFER+BUFFER_SIZE); ckb_debug(message);
        for (int i=0; i<BUFFER_SIZE; i++) {
            CODE_BUFFER[i] += 1;
        }
    }

    sprintf(message, "argv[1] = %s", argv[1]); ckb_debug(message);
    if (strlen(argv[1]) != 8*2) {
        return -13;
    }
    uint64_t recursion = read_u64_le_from_hex(argv[1]);

    sprintf(message, "argv[2] = %s", argv[2]); ckb_debug(message);
    if (strlen(argv[2]) != 8*2) {
        return -14;
    }
    uint64_t number = read_u64_le_from_hex(argv[2]);

    if (recursion > 0) {
        try_exec(argv, recursion-1, number-1);
    }

    sprintf(message, "argv[3] = %s", argv[3]); ckb_debug(message);
    if (strlen(argv[3]) != 8*2) {
        return -15;
    }
    uint64_t expected = read_u64_le_from_hex(argv[3]);

    bool if_load_after_exec  = (flag & 0b0100) == 0b0100;
    if (if_load_after_exec) {
        sprintf(message, "argv[8] = %s", argv[8]); ckb_debug(message);
        if (strlen(argv[8]) != 32*2) {
            return -16;
        }
        uint8_t code_hash[32];
        from_hex(code_hash, argv[8], 32);
        int ret = try_load_code(&number, code_hash);
        if (ret != CKB_SUCCESS) {
            return ret;
        }
        sprintf(message, "(apply after exec) number = %ld", number); ckb_debug(message);
    }

    if (number == expected) {
        return CKB_SUCCESS;
    } else {
        return -17;
    }
}
