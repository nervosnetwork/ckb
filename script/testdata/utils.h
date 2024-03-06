
#ifndef __UTILS_H__
#define __UTILS_H__

#include <stdio.h>

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

// conventions
#define CKB_STDIN (0)
#define CKB_STDOUT (1)

// mimic stdio pipes on linux
int create_std_pipes(uint64_t* fds, uint64_t* inherited_fds) {
    printf("entering create_std_pipes");
    int err = 0;

    uint64_t to_child[2] = {0};
    uint64_t to_parent[2] = {0};
    printf("call ckb_pipe");
    err = ckb_pipe(to_child);
    CHECK(err);
    printf("call ckb_pipe");
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
