#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

uint64_t read_u64_le(const uint8_t* src) {
    return *(const uint64_t*)src;
}

int main() {
    int err = 0;
    uint64_t n = 0;

    uint8_t args[32] = {0};
    n = countof(args);
    err = load_script_args(args, &n);
    CHECK(err);
    CHECK2(n == 32, ErrorCommon);
    uint64_t args_index = read_u64_le(&args[0x00]);
    uint64_t args_source = read_u64_le(&args[0x08]);
    uint64_t args_place = read_u64_le(&args[0x10]);
    uint64_t args_bounds = read_u64_le(&args[0x18]);
    printf("args.index  = %llu", args_index);
    printf("args.source = %llu", args_source);
    printf("args.place  = %llu", args_place);
    printf("args.bounds = %llu", args_bounds);

    const char *argv[] = {};
    uint64_t pid = 0;
    uint64_t fds[2] = {0};
    uint64_t inherited_fds[3] = {0};
    err = create_std_pipes(fds, inherited_fds);
    CHECK(err);

    spawn_args_t spgs = {
        .argc = countof(argv),
        .argv = argv,
        .process_id = &pid,
        .inherited_fds = inherited_fds,
    };
    err = ckb_spawn(args_index, args_source, args_place, args_bounds, &spgs);
    CHECK(err);

    size_t length = 0;
    length = 12;
    err = ckb_write(fds[CKB_STDOUT], "Hello World!", &length);
    CHECK(err);
    err = ckb_close(fds[CKB_STDOUT]);
    CHECK(err);

    uint8_t buffer[1024] = {0};
    length = 1024;
    err = ckb_read_all(fds[CKB_STDIN], buffer, &length);
    CHECK(err);
    CHECK2(length == 12, ErrorCommon);
    err = memcmp("Hello World!", buffer, length);
    CHECK(err);

exit:
    return err;
}
