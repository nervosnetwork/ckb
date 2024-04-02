
#ifndef __UTILS_H__
#define __UTILS_H__

#include <stdio.h>
#include <stdbool.h>

#include "ckb_consts.h"
#include "ckb_syscalls.h"
#include "blockchain.h"

enum CkbSpawnError {
    ErrorCommon = 31,
    ErrorRead,
    ErrorWrite,
    ErrorPipe,
    ErrorSpawn,
};

#define CHECK2(cond, code)                                                     \
    do {                                                                       \
        if (!(cond)) {                                                         \
            printf("error at %s:%d, error code %d", __FILE__, __LINE__, code); \
            err = code;                                                        \
            goto exit;                                                         \
        }                                                                      \
    } while (0)

#define CHECK(_code)                                                           \
    do {                                                                       \
        int code = (_code);                                                    \
        if (code != 0) {                                                       \
            printf("error at %s:%d, error code %d", __FILE__, __LINE__, code); \
            err = code;                                                        \
            goto exit;                                                         \
        }                                                                      \
    } while (0)
#endif

#define countof(array) (sizeof(array) / sizeof(array[0]))
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define MIN(a, b) ((a) < (b) ? (a) : (b))

// conventions
#define CKB_STDIN (0)
#define CKB_STDOUT (1)

// mimic stdio pipes on linux
int create_std_pipes(uint64_t* fds, uint64_t* inherited_fds) {
    int err = 0;

    uint64_t to_child[2] = {0};
    uint64_t to_parent[2] = {0};
    err = ckb_pipe(to_child);
    CHECK(err);
    err = ckb_pipe(to_parent);
    CHECK(err);

    inherited_fds[0] = to_child[0];
    inherited_fds[1] = to_parent[1];
    inherited_fds[2] = 0;

    fds[CKB_STDIN] = to_parent[0];
    fds[CKB_STDOUT] = to_child[1];

exit:
    return err;
}

// spawn script at `index` in cell_deps without any argc, argv
int simple_spawn(size_t index) {
    int err = 0;
    int8_t spawn_exit_code = 255;
    const char* argv[1] = {0};
    uint64_t pid = 0;
    uint64_t fds[1] = {0};
    spawn_args_t spgs = {.argc = 0, .argv = argv, .process_id = &pid, .inherited_fds = fds};
    err = ckb_spawn(index, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);
    err = ckb_wait(pid, &spawn_exit_code);
    CHECK(err);
    CHECK(spawn_exit_code);

exit:
    return err;
}

// spawn script at `index` in cell_deps with argv
int simple_spawn_args(size_t index, int argc, const char* argv[]) {
    int err = 0;
    int8_t spawn_exit_code = 255;
    uint64_t pid = 0;
    uint64_t fds[1] = {0};
    spawn_args_t spgs = {.argc = argc, .argv = argv, .process_id = &pid, .inherited_fds = fds};
    err = ckb_spawn(index, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);
    err = ckb_wait(pid, &spawn_exit_code);
    CHECK(err);
    CHECK(spawn_exit_code);
exit:
    return err;
}

int full_spawn(size_t index, int argc, const char* argv[], uint64_t fds[2], uint64_t* pid) {
    int err = 0;
    uint64_t inherited_fds[3] = {0};
    err = create_std_pipes(fds, inherited_fds);
    CHECK(err);
    spawn_args_t spgs = {.argc = argc, .argv = argv, .process_id = pid, .inherited_fds = inherited_fds};
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);
exit:
    return err;
}

// read exact `length` bytes into buffer.
// Will wait forever when less bytes are written on write fd.
int read_exact(uint64_t fd, void* buffer, size_t length, size_t* actual_length) {
    int err = 0;
    size_t remaining_length = length;
    uint8_t* start_buffer = buffer;
    while (true) {
        size_t n = remaining_length;
        err = ckb_read(fd, start_buffer, &n);
        if (err == CKB_OTHER_END_CLOSED) {
            err = 0;
            break;
        } else {
            CHECK(err);
        }
        start_buffer += n;
        remaining_length -= n;
        *actual_length = length - remaining_length;
        if (remaining_length == 0) {
            break;
        }
    }

exit:
    return err;
}

// Function read_all reads from fd until an error or EOF and returns the data it read.
int ckb_read_all(uint64_t fd, void* buffer, size_t* length) {
    int err = 0;
    size_t read_length = 0;
    size_t full_length = *length;
    uint8_t* b = buffer;
    while (true) {
        size_t n = full_length - read_length;
        err = ckb_read(fd, b, &n);
        if (err == CKB_OTHER_END_CLOSED) {
            err = 0;
            *length = read_length;
            break;
        } else {
            CHECK(err);
        }
        if (full_length - read_length == 0) {
            err = CKB_LENGTH_NOT_ENOUGH;
            CHECK(err);
        }
        b += n;
        read_length += n;
        *length = read_length;
    }

exit:
    return err;
}

// write exact `length` bytes into buffer.
// Will wait forever when less bytes are read on read fd.
int write_exact(uint64_t fd, void* buffer, size_t length, size_t* actual_length) {
    int err = 0;
    size_t remaining_length = length;
    uint8_t* start_buffer = buffer;
    while (true) {
        size_t n = remaining_length;
        err = ckb_write(fd, start_buffer, &n);
        if (err == CKB_OTHER_END_CLOSED) {
            err = 0;
            break;
        } else {
            CHECK(err);
        }
        start_buffer += n;
        remaining_length -= n;
        *actual_length = length - remaining_length;
        if (remaining_length == 0) {
            break;
        }
    }
exit:
    return err;
}

#define SCRIPT_SIZE 4096

int load_script_args(uint8_t* args, size_t* length) {
    int err = 0;
    uint64_t len = SCRIPT_SIZE;
    uint8_t script[SCRIPT_SIZE];
    err = ckb_load_script(script, &len, 0);
    CHECK(err);
    CHECK2(len <= SCRIPT_SIZE, -2);
    mol_seg_t script_seg = {0};
    script_seg.ptr = (uint8_t*)script;
    script_seg.size = len;
    CHECK2(MolReader_Script_verify(&script_seg, false) == MOL_OK, -3);
    mol_seg_t args_seg = MolReader_Script_get_args(&script_seg);
    mol_seg_t bytes_seg = MolReader_Bytes_raw_bytes(&args_seg);
    size_t copy_length = MIN(bytes_seg.size, *length);
    memcpy(args, bytes_seg.ptr, copy_length);
    *length = copy_length;

exit:
    return err;
}

void print_hex(const uint8_t* buf, size_t length) {
    char dst[65] = {0};
    int j = 0;
    for (int i = 0; i < length; i++) {
        char hi = buf[i] >> 4;
        char lo = buf[i] & 0xf;
        dst[j * 2] = hi + (hi < 10 ? '0' : ('a' - 10));
        dst[j * 2 + 1] = lo + (lo < 10 ? '0' : ('a' - 10));
        j += 1;
        if (j > 31) {
            j = 0;
            printf("%s", dst);
        }
    }
    if (j != 0) {
        dst[j * 2] = 0;
        printf("%s", dst);
    }
}
