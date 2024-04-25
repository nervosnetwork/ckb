#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

#define BUFFER_SIZE 1024 * 4

typedef struct {
    uint64_t io_size;
    bool check_buffer;
} ScriptArgs;

int parent(ScriptArgs* args, uint8_t* buffer) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    uint64_t pid = 0;
    err = full_spawn(0, 1, argv, fds, &pid);
    CHECK(err);

    uint64_t buf_len = args->io_size;

    err = ckb_read(fds[CKB_STDIN], buffer, &buf_len);
    CHECK(err);
    CHECK2(buf_len == args->io_size, -1);
    if (args->check_buffer) {
        for (size_t i = 0; i < args->io_size; i++)
            CHECK2(buffer[i] == (uint8_t)i, -1);
    }

    int8_t exit_code = 0;
    err = ckb_wait(pid, &exit_code);
    CHECK(err);
    CHECK(exit_code);

exit:
    return err;
}

int child(ScriptArgs* args, uint8_t* buffer) {
    int err = 0;
    uint64_t inherited_fds[2];
    size_t inherited_fds_length = 2;
    err = ckb_inherited_file_descriptors(inherited_fds, &inherited_fds_length);
    CHECK(err);

    uint64_t buf_len = args->io_size;

    if (args->check_buffer) {
        for (size_t i = 0; i < args->io_size; i++) buffer[i] = i;
    }

    err = ckb_write(inherited_fds[CKB_STDOUT], buffer, &buf_len);

    CHECK(err);
    CHECK2(buf_len == args->io_size, -1);
exit:
    return err;
}

int main() {
    int err = 0;
    ScriptArgs script_args;
    size_t script_args_length = sizeof(script_args);
    err = load_script_args((uint8_t*)&script_args, &script_args_length);
    CHECK(err);
    CHECK2(script_args_length == sizeof(script_args), -1);

    uint64_t cid = ckb_process_id();
    uint8_t buffer[BUFFER_SIZE] = {0};

    if (cid == 0) {
        return parent(&script_args, buffer);
    } else {
        return child(&script_args, buffer);
    }

exit:
    return err;
}
