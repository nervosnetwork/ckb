#define CKB_C_STDLIB_PRINTF 1
#include <ckb_syscalls.h>
#include <stdio.h>

#define WRITE_TIMES 10
#define SPAWN_TIMES 17

void print_current_cycle() { printf("id: %lu,cycle: %lu\n", ckb_process_id(), ckb_current_cycles()); }

int child() {
    uint64_t std_fds[2];
    size_t length = 2;

    int ret = ckb_inherited_fds(std_fds, &length);
    if (ret != CKB_SUCCESS) {
        return ret;
    }
    if (length != 2) {
        printf("Invalid number of fds!\n");
        return -1;
    }
    printf("Inherited fds: %lu %lu\n", std_fds[0], std_fds[1]);
    print_current_cycle();

    for (int i = 0; i < WRITE_TIMES; i++) {
        uint8_t data[4] = {(uint8_t)ckb_process_id(), (uint8_t)ckb_process_id(), (uint8_t)ckb_process_id(),
                           (uint8_t)ckb_process_id()};
        length = 4;
        ret = ckb_write(std_fds[1], data, &length);
        if (ret == CKB_SUCCESS) {
            printf("[spawn] write length: %lu\n", length);
        } else {
            printf("[spawn] write failed result: %d\n", ret);
        }
        print_current_cycle();
        printf("----read data----\n");

        uint8_t read[4] = {0};
        length = 4;
        ret = ckb_read(std_fds[0], read, &length);
        if (ret == CKB_SUCCESS) {
            printf("read fd: %lu, data: %d %d %d %d, length: %lu\n", std_fds[0], read[0], read[1], read[2], read[3],
                   length);
        } else {
            printf("read fd: %lu err: %d\n", std_fds[0], ret);
        }
    }

    printf("finished\n");
    return (int)ckb_process_id();
}

int root() {
    uint64_t root_process_write_fds[SPAWN_TIMES] = {0};
    uint64_t root_process_read_fds[SPAWN_TIMES] = {0};
    int spawns = 0;
    int ret;
    size_t length;

    for (int i = 0; i < SPAWN_TIMES; i++) {
        printf("current i: %d\n", i);
        uint64_t root_read_spawn_write_pipe[2];
        uint64_t spawn_read_root_write_pipe[2];
        /*
         * Note that more than permitted amount of pipes will be created.
         * We will ignore the errors here since this is just a dummy test.
         */
        ret = ckb_pipe(root_read_spawn_write_pipe);
        if (ret != CKB_SUCCESS) {
            root_read_spawn_write_pipe[0] = 0;
            root_read_spawn_write_pipe[1] = 0;
        }
        ret = ckb_pipe(spawn_read_root_write_pipe);
        if (ret != CKB_SUCCESS) {
            spawn_read_root_write_pipe[0] = 0;
            spawn_read_root_write_pipe[1] = 0;
        }
        printf("root_read: %lu, spawn_write: %lu, spawn_read: %lu, root_write: %lu\n", root_read_spawn_write_pipe[0],
               root_read_spawn_write_pipe[1], spawn_read_root_write_pipe[0], spawn_read_root_write_pipe[1]);

        print_current_cycle();
        uint64_t inherited_fds[3] = {spawn_read_root_write_pipe[0], root_read_spawn_write_pipe[1], 0};
        uint64_t pid = (uint64_t)-1;
        spawn_args_t args = {
            .argc = 0,
            .argv = NULL,
            .process_id = &pid,
            .inherited_fds = inherited_fds,
        };
        ret = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &args);
        if (ret == CKB_SUCCESS) {
            printf("invoke spawn: %d process id: %lu\n", i, pid);
            root_process_read_fds[i] = root_read_spawn_write_pipe[0];
            root_process_write_fds[i] = spawn_read_root_write_pipe[1];
            if (i + 1 != pid) {
                printf("Unexpected process id!\n");
                return -1;
            }
            spawns = i + 1;
        } else {
            printf("invoke spawn: %d err: %d\n", i, ret);
            if ((i < 16) || (ret != CKB_MAX_VMS_SPAWNED)) {
                printf("Unexpected spawn error!\n");
                return -1;
            }
        }
    }

    printf("write data\n");
    for (int i = 0; i < WRITE_TIMES; i++) {
        for (int j = 0; j < spawns; j++) {
            uint64_t read_fd = root_process_read_fds[j];
            uint64_t write_fd = root_process_write_fds[j];

            uint8_t read[4] = {0};
            length = 4;
            ret = ckb_read(read_fd, read, &length);
            if (ret == CKB_SUCCESS) {
                printf("root read fd: %lu, length: %lu\n", read_fd, length);
                uint8_t expected[4] = {j + 1, j + 1, j + 1, j + 1};
                if (memcmp(expected, read, 4) != 0) {
                    printf("Read corrupted data!\n");
                    return -1;
                }
            } else {
                printf("root read fd: %lu, err: %d\n", read_fd, ret);
            }

            uint8_t write[4] = {0};
            length = 4;
            ret = ckb_write(write_fd, write, &length);
            if (ret == CKB_SUCCESS) {
                printf("root write fd: %lu, length: %lu\n", write_fd, length);
            } else {
                printf("root write fd: %lu, err: %d\n", write_fd, ret);
            }
        }
    }

    for (int i = 1; i < SPAWN_TIMES; i++) {
        int8_t exit_code;
        ret = ckb_wait(i, &exit_code);
        if (ret == CKB_SUCCESS) {
            printf("root wait %lu, exit code: %d\n", i, exit_code);
        } else {
            printf("root wait %lu, err: %d\n", i, ret);
        }
    }

    for (int i = 0; i < spawns; i++) {
        uint64_t read_fd = root_process_read_fds[i];
        uint8_t read[4] = {0};
        length = 4;
        ret = ckb_read(read_fd, read, &length);
        if (ret == CKB_SUCCESS) {
            printf("root read fd: %lu, length: %lu\n", read_fd, length);
        } else {
            printf("root read fd: %lu, err: %d\n", read_fd, ret);
        }

        uint64_t write_fd = root_process_write_fds[i];
        uint8_t write[4] = {0};
        length = 4;
        ret = ckb_write(write_fd, write, &length);
        if (ret == CKB_SUCCESS) {
            printf("root write fd: %lu, length: %lu\n", write_fd, length);
        } else {
            printf("root write fd: %lu, err: %d\n", write_fd, ret);
        }
    }

    return 0;
}

int main() {
    if (ckb_process_id() > 0) {
        return child();
    }
    return root();
}
