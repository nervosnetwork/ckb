#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

int main() {
    int err = 0;

    uint64_t fds[2] = {0};
    uint64_t fds_len = countof(fds);
    err = ckb_inherited_fds(fds, &fds_len);
    CHECK(err);
    CHECK2(fds_len == 2, ErrorCommon);

    uint8_t buffer[1024] = {0};
    size_t length;
    length = 1024;
    ckb_read_all(fds[CKB_STDIN], buffer, &length);
    CHECK2(length == 12, ErrorCommon);

    err = ckb_write(fds[CKB_STDOUT], buffer, &length);
    CHECK(err);
    err = ckb_close(fds[CKB_STDOUT]);
    CHECK(err);

exit:
    return err;
}
