#include "spawn_utils.h"

typedef enum SyscallId { SyscallRead, SyscallWrite, SyscallClose } SyscallId;

typedef struct Command {
    SyscallId id;
    uint64_t buf_ptr;
    uint64_t len_ptr;
    size_t fd_index;
} Command;

typedef struct Data {
    uint8_t* ptr;
    uint64_t offset;
    uint64_t total_size;
} Data;

int extract_command(Data* data, Command* cmd) {
    if ((data->offset + 1) > data->total_size) {
        return -1;
    }
    uint8_t id = data->ptr[0];

    if (id > 250) {
        cmd->id = SyscallClose;
        cmd->fd_index = (size_t)(id % 2);
        data->offset += 1;
    } else if (id > 128) {
        if ((data->offset + 7) > data->total_size) {
            return -1;
        }
        cmd->id = SyscallRead;
        memcpy(&cmd->buf_ptr, &data->ptr[data->offset + 1], 3);
        memcpy(&cmd->len_ptr, &data->ptr[data->offset + 4], 3);
        data->offset += 7;
    } else {
        if ((data->offset + 7) > data->total_size) {
            return -1;
        }
        cmd->id = SyscallWrite;
        memcpy(&cmd->buf_ptr, &data->ptr[data->offset + 1], 3);
        memcpy(&cmd->len_ptr, &data->ptr[data->offset + 4], 3);
        data->offset += 7;
    }
    return 0;
}

int random_read_write(uint64_t fds[2], size_t index) {
    int err = 0;
    uint8_t cmd_buf[4096] = {0};
    uint64_t cmd_len = sizeof(cmd_buf);

    err = ckb_load_witness(cmd_buf, &cmd_len, 0, index, CKB_SOURCE_INPUT);
    CHECK(err);
    Data data = {.ptr = cmd_buf, .total_size = cmd_len, .offset = 0};

    while (true) {
        Command cmd = {0};
        err = extract_command(&data, &cmd);
        if (err) break;
        if (cmd.id == SyscallRead) {
            ckb_read(fds[CKB_STDIN], (void*)cmd.buf_ptr, (uint64_t*)cmd.len_ptr);
            // ignore error
        } else if (cmd.id == SyscallWrite) {
            ckb_write(fds[CKB_STDOUT], (void*)cmd.buf_ptr, (uint64_t*)cmd.len_ptr);
            // ignore error
        } else if (cmd.id == SyscallClose) {
            ckb_close(fds[cmd.fd_index]);
            // ignore error
        } else {
            CHECK2(false, -1);
        }
    }
exit:
    return err;
}

int parent_entry() {
    int err = 0;
    uint64_t pid = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};

    err = full_spawn(0, 1, argv, fds, &pid);
    CHECK(err);
    random_read_write(fds, 0);

    int8_t exit_code = 0;
    err = ckb_wait(pid, &exit_code);
    CHECK(err);
    CHECK(exit_code);
exit:
    return err;
}

int child_entry() {
    int err = 0;
    uint64_t inherited_fds[2];
    size_t inherited_fds_length = 2;
    err = ckb_inherited_fds(inherited_fds, &inherited_fds_length);
    CHECK(err);
    random_read_write(inherited_fds, 0);

exit:
    return err;
}

int main(int argc, const char* argv[]) {
    if (argc > 0) {
        return child_entry();
    } else {
        return parent_entry();
    }
}
