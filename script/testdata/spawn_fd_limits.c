#include <stdint.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

int main() {
    int err = 0;
    uint64_t pipe[2] = {0};
    for (int i = 0; i < 32; i++) {
        err = ckb_pipe(pipe);
        CHECK(err);
    }
    // Create up to 64 pipes.
    err = ckb_pipe(pipe);
    err = err - CKB_MAX_FDS_CREATED;

exit:
    return err;
}
