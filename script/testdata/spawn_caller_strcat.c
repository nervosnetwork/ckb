#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

int main() {
    int err = 0;
    const char *argv[] = {"hello", "world"};
    uint64_t pid = 0;
    uint64_t fds[2] = {0};
    uint64_t inherited_fds[3] = {0};
    err = create_std_fds(fds, inherited_fds);
    CHECK(err);

    spawn_args_t spgs = {
        .argc = 2,
        .argv = argv,
        .process_id = &pid,
        .inherited_fds = inherited_fds,
    };
    err = ckb_spawn(1, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);
    uint8_t buffer[1024] = {0};
    size_t length = 1024;
    err = ckb_read(fds[CKB_STDIN], buffer, &length);
    CHECK(err);
    err = memcmp("helloworld", buffer, length);
    CHECK(err);

exit:
    return err;
}
