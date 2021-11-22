/*  Script Description:
 *  - Args:
 *    - Two little endian unsigned integer: num0 and num1.
 *    - A list of `code_hash`(`data_hash`) of shared libraries which contains the method `uint64_t apply(uint64_t)`.
 *  - Returns `CKB_SUCCESS` if and only if any follow conditions satisfied:
 *    - `num0 == num1` is zero at start.
 *    - `num0 == num1` is zero at last.
 */

#include "ckb_dlfcn.h"
#include "ckb_syscalls.h"
#include "blockchain.h"

#ifdef DEBUG
#include <stdio.h>
#else
#define ckb_debug(...)
#define sprintf(...)
#endif

#define SCRIPT_SIZE 32768
#define CODE_BUFFER_SIZE (1024 * 32)
#define CACHE_CAPACITY 4

typedef uint64_t(arithmetic_func_t) (uint64_t);

void try_pause() {
    syscall(2178, 0, 0, 0, 0, 0, 0);
}

uint64_t read_u64_le (const uint8_t *src) {
    return *(const uint64_t *)src;
}

int load_arithmetic_func (arithmetic_func_t** func, uint8_t* code_hash, uint8_t* code_buffer) {
    void *handle = NULL;
    uint64_t consumed_size = 0;
    uint8_t hash_type = 0;
    int ret = ckb_dlopen2(code_hash, hash_type, code_buffer, CODE_BUFFER_SIZE, &handle, &consumed_size);
    if (ret != CKB_SUCCESS) {
        return -11;
    }
    *func = (arithmetic_func_t*) ckb_dlsym(handle, "apply");
    if (*func == NULL) {
        return -12;
    }
    return CKB_SUCCESS;
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

    if ((bytes_seg.size - 8 * 2) % 32 != 0) {
        return -4;
    }

    volatile uint64_t num0 = read_u64_le(bytes_seg.ptr);
    volatile uint64_t num1 = read_u64_le(bytes_seg.ptr+8);
    sprintf(message, "before num0 = %ld, num1 = %ld", num0, num1); ckb_debug(message);

    if (num0 == num1) {
        return CKB_SUCCESS;
    }

    int64_t total_funcs_count = (bytes_seg.size - 8 * 2) / 32;
    int64_t called_funcs_count = 0;
    uint8_t* code_hash_ptr = bytes_seg.ptr + 8 * 2;

    int cache_size = 0;

    uint8_t* cached_code_hash[CACHE_CAPACITY];
    for (int i = 0; i < CACHE_CAPACITY; i++) {
        cached_code_hash[i] = NULL;
    }

    void (*cached_funcs[CACHE_CAPACITY])();
    arithmetic_func_t* tmp_func = NULL;

    uint8_t cached_code_buffer_0[CODE_BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));
    uint8_t cached_code_buffer_1[CODE_BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));
    uint8_t cached_code_buffer_2[CODE_BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));
    uint8_t cached_code_buffer_3[CODE_BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));

    uint8_t tmp_code_buffer[CODE_BUFFER_SIZE] __attribute__((aligned(RISCV_PGSIZE)));

    while (called_funcs_count < total_funcs_count) {
        bool is_found = false;
        if (cache_size > 0) {
            for (int i = 0; i < cache_size; i++) {
                if (0 == memcmp(code_hash_ptr, cached_code_hash[i], 32)) {
                    // Find the function from caches.
                    tmp_func = (arithmetic_func_t*) cached_funcs[i];
                    is_found = true;
                }
            }
        }
        if (!is_found) {
            uint8_t* code_buffer = NULL;
            switch (cache_size) {
                case 0:
                    code_buffer = cached_code_buffer_0;
                    break;
                case 1:
                    code_buffer = cached_code_buffer_1;
                    break;
                case 2:
                    code_buffer = cached_code_buffer_2;
                    break;
                case 3:
                    code_buffer = cached_code_buffer_3;
                    break;
                default:
                    code_buffer = tmp_code_buffer;
                    break;
            }
            ret = load_arithmetic_func(&tmp_func, code_hash_ptr, code_buffer);
            if (ret != CKB_SUCCESS) {
                return ret;
            }
            // Cache the current function.
            if (cache_size < CACHE_CAPACITY) {
                cached_code_hash[cache_size] = code_hash_ptr;
                cached_funcs[cache_size] = (void*) tmp_func;
                cache_size += 1;
            }
        }

        try_pause();
        num0 = tmp_func(num0);

        code_hash_ptr += 32;
        called_funcs_count += 1;
    }

    sprintf(message, "after  num0 = %ld, num1 = %ld", num0, num1); ckb_debug(message);

    if (num0 != num1) {
        return -5;
    }

    return CKB_SUCCESS;
}
