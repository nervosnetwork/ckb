#include "spawn_utils.h"

int parent_simple_read_write(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};

    err = full_spawn(0, 1, argv, fds, pid);
    // write
    uint8_t block[11] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
    for (size_t i = 0; i < 7; i++) {
        size_t actual_length = 0;
        err = write_exact(fds[CKB_STDOUT], block, sizeof(block), &actual_length);
        CHECK(err);
        CHECK2(actual_length == sizeof(block), -2);
    }
    // read
    for (size_t i = 0; i < 7; i++) {
        uint8_t block[11] = {0};
        size_t actual_length = 0;
        err = read_exact(fds[CKB_STDIN], block, sizeof(block), &actual_length);
        CHECK(err);
        CHECK2(actual_length == sizeof(block), -2);
        for (size_t j = 0; j < sizeof(block); j++) {
            CHECK2(block[j] == 0xFF, -2);
        }
    }
exit:
    return err;
}

int child_simple_read_write() {
    int err = 0;
    uint64_t inherited_fds[2];
    size_t inherited_fds_length = 2;
    err = ckb_inherited_file_descriptors(inherited_fds, &inherited_fds_length);
    // read
    for (size_t i = 0; i < 11; i++) {
        uint8_t block[7] = {0};
        size_t actual_length = 0;
        err = read_exact(inherited_fds[CKB_STDIN], block, sizeof(block), &actual_length);
        CHECK(err);
        CHECK2(actual_length == sizeof(block), -2);
        for (size_t j = 0; j < sizeof(block); j++) {
            CHECK2(block[j] == 0xFF, -3);
        }
    }
    // write
    uint8_t block[11] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
    for (size_t i = 0; i < 7; i++) {
        size_t actual_length = 0;
        err = write_exact(inherited_fds[CKB_STDOUT], block, sizeof(block), &actual_length);
        CHECK(err);
        CHECK2(actual_length == sizeof(block), -2);
    }
exit:
    return err;
}

int parent_write_dead_lock(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    err = full_spawn(0, 1, argv, fds, pid);
    CHECK(err);
    uint8_t data[10];
    size_t data_length = sizeof(data);
    err = ckb_write(fds[CKB_STDOUT], data, &data_length);
    CHECK(err);

exit:
    return err;
}

int child_write_dead_lock() {
    int err = 0;
    uint64_t inherited_fds[3] = {0};
    size_t inherited_fds_length = 3;
    err = ckb_inherited_file_descriptors(inherited_fds, &inherited_fds_length);
    CHECK(err);
    uint8_t data[10];
    size_t data_length = sizeof(data);
    err = ckb_write(inherited_fds[CKB_STDOUT], data, &data_length);
    CHECK(err);
exit:
    return err;
}

int parent_invalid_fd(uint64_t* pid) {
    uint64_t invalid_fd = 0xff;
    uint8_t data[4];
    size_t data_length = sizeof(data);
    int err = ckb_read(invalid_fd, data, &data_length);
    CHECK2(err != 0, -2);
    err = ckb_write(invalid_fd, data, &data_length);
    CHECK2(err != 0, -2);

    uint64_t fds[2] = {0};
    err = ckb_pipe(fds);
    // read on write fd
    err = ckb_read(fds[CKB_STDOUT], data, &data_length);
    CHECK2(err != 0, -3);
    // write on read fd
    err = ckb_write(fds[CKB_STDIN], data, &data_length);
    CHECK2(err != 0, -3);

    // pass fd to child to make it invalid
    const char* argv[] = {"", 0};
    uint64_t inherited_fds[2] = {fds[0], 0};
    spawn_args_t spgs = {.argc = 1, .argv = argv, .process_id = pid, .inherited_fds = inherited_fds};
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);
    err = ckb_read(fds[0], data, &data_length);
    CHECK2(err != 0, -3);

    // write to fd but the other end is closed
    err = ckb_pipe(fds);
    CHECK(err);
    err = ckb_close(fds[CKB_STDIN]);
    CHECK(err);
    err = ckb_write(fds[CKB_STDOUT], data, &data_length);
    CHECK2(err == CKB_OTHER_END_CLOSED, -2);

    // read from fd but the ohter end is closed
    err = ckb_pipe(fds);
    CHECK(err);
    err = ckb_close(fds[CKB_STDOUT]);
    CHECK(err);
    err = ckb_read(fds[CKB_STDIN], data, &data_length);
    CHECK2(err == CKB_OTHER_END_CLOSED, -2);
    err = 0;
exit:
    return err;
}

int parent_wait_dead_lock(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    err = full_spawn(0, 1, argv, fds, pid);
    CHECK(err);

exit:
    return err;
}

int child_wait_dead_lock() {
    uint64_t pid = 0;  // parent pid
    int8_t exit_code = 0;
    return ckb_wait(pid, &exit_code);
}

int parent_read_write_with_close(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    err = full_spawn(0, 1, argv, fds, pid);
    // write util the other end is closed
    uint8_t block[100];
    for (size_t i = 0; i < sizeof(block); i++) {
        block[i] = 0xFF;
    }
    size_t actual_length = 0;
    err = write_exact(fds[CKB_STDOUT], block, sizeof(block), &actual_length);
    CHECK(err);
    CHECK2(actual_length == sizeof(block), -2);

    err = 0;
exit:
    return err;
}

int child_read_write_with_close() {
    int err = 0;
    uint64_t inherited_fds[2];
    size_t inherited_fds_length = 2;
    err = ckb_inherited_file_descriptors(inherited_fds, &inherited_fds_length);
    CHECK(err);

    // read 100 bytes and close
    uint8_t block[100] = {0};
    size_t actual_length = 0;
    err = read_exact(inherited_fds[CKB_STDIN], block, sizeof(block), &actual_length);
    CHECK(err);
    CHECK2(actual_length == sizeof(block), -2);
    for (size_t j = 0; j < sizeof(block); j++) {
        CHECK2(block[j] == 0xFF, -3);
    }
    err = ckb_close(inherited_fds[CKB_STDIN]);
    CHECK(err);

exit:
    return err;
}

int parent_wait_multiple(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    full_spawn(0, 1, argv, fds, pid);
    CHECK(err);

    int8_t exit_code = 0;
    err = ckb_wait(*pid, &exit_code);
    CHECK(err);
    // second wait is not allowed
    err = ckb_wait(*pid, &exit_code);
    CHECK2(err != 0, -2);
    err = 0;
    // spawn a new valid one, make ckb_wait happy
    full_spawn(0, 1, argv, fds, pid);
    CHECK(err);

exit:
    return err;
}

int parent_inherited_fds(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t inherited_fds[11] = {0};
    for (size_t i = 0; i < 5; i++) {
        err = ckb_pipe(&inherited_fds[i * 2]);
        CHECK(err);
    }
    spawn_args_t spgs = {.argc = 1, .argv = argv, .process_id = pid, .inherited_fds = inherited_fds};
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);
exit:
    return err;
}

int child_inherited_fds() {
    int err = 0;

    // correct way to get fd length
    size_t fds_length = 0;
    err = ckb_inherited_file_descriptors(0, &fds_length);
    CHECK2(fds_length == 10, -2);

    // wrong way to get fd length
    fds_length = 2;
    err = ckb_inherited_file_descriptors(0, &fds_length);
    CHECK2(err != 0, -2);

    // get part of fds
    uint64_t fds[11] = {0};
    fds_length = 1;
    err = ckb_inherited_file_descriptors(fds, &fds_length);
    CHECK(err);
    CHECK2(fds_length == 10, -2);
    CHECK2(fds[0] == 2, -2);

    // get all fds
    fds_length = 10;
    err = ckb_inherited_file_descriptors(fds, &fds_length);
    CHECK(err);
    CHECK2(fds_length == 10, -2);
    for (size_t i = 0; i < 10; i++) {
        CHECK2(fds[i] == (i + 2), -2);
    }
exit:
    return err;
}

int parent_inherited_fds_without_owner(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[3] = {0xFF, 0xEF, 0};

    spawn_args_t spgs = {.argc = 1, .argv = argv, .process_id = pid, .inherited_fds = fds};
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK2(err == CKB_INVALID_PIPE, -2);

    // create valid fds
    err = ckb_pipe(fds);
    CHECK(err);
    // then transferred by spawn
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK(err);

    // the fds are already transferred. An error expected.
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
    CHECK2(err == CKB_INVALID_PIPE, -2);
    err = 0;
exit:
    return err;
}

int parent_read_then_close(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    err = full_spawn(0, 1, argv, fds, pid);
    CHECK(err);
    err = ckb_close(fds[CKB_STDOUT]);
    CHECK(err);
exit:
    return err;
}

int child_read_then_close() {
    int err = 0;
    uint64_t fds[2] = {0};
    uint64_t fds_length = 2;
    err = ckb_inherited_file_descriptors(fds, &fds_length);
    CHECK(err);
    uint8_t data[8];
    size_t data_len = sizeof(data);
    // first read to return 0 byte without error
    err = ckb_read(fds[CKB_STDIN], data, &data_len);
    CHECK(err);
    CHECK2(data_len == 0, -2);
    // second read to return error(other end closed)
    err = ckb_read(fds[CKB_STDIN], data, &data_len);
    CHECK2(err == CKB_OTHER_END_CLOSED, -2);

    err = 0;
exit:
    return err;
}

int parent_max_vms_count() {
    const char* argv[2] = {"", 0};
    return simple_spawn_args(0, 1, argv);
}

int child_max_vms_count() {
    const char* argv[2] = {"", 0};
    int err = simple_spawn_args(0, 1, argv);
    CHECK2(err == 0 || err == CKB_MAX_VMS_SPAWNED, -2);
    err = 0;
exit:
    return err;
}

int parent_max_pipe_limits() {
    const char* argv[2] = {"", 0};
    int err = 0;
    uint64_t fd[2] = {0};
    for (int i = 0; i < 16; i++) {
        err = ckb_pipe(fd);
        CHECK(err);
    }
    err = simple_spawn_args(0, 1, argv);
exit:
    return err;
}

int child_max_pipe_limits() {
    int err = 0;
    uint64_t pipe[2] = {0};
    for (int i = 0; i < 16; i++) {
        err = ckb_pipe(pipe);
        CHECK(err);
    }
    // Create up to 64 pipes.
    err = ckb_pipe(pipe);
    err = err - 9;

exit:
    return err;
}

int parent_close_invalid_fd() {
    uint64_t fds[2] = {0};
    int err = ckb_pipe(fds);
    CHECK(err);

    err = ckb_close(fds[CKB_STDIN] + 32);
    CHECK2(err == 6, -1);

    err = ckb_close(fds[CKB_STDIN]);
    CHECK(err);
    err = ckb_close(fds[CKB_STDOUT]);
    CHECK(err);

    err = ckb_close(fds[CKB_STDIN]);
    CHECK2(err == 6, -1);
    err = ckb_close(fds[CKB_STDOUT]);
    CHECK2(err == 6, -1);

    err = 0;
exit:
    return err;
}

int parent_write_closed_fd(uint64_t* pid) {
    int err = 0;
    const char* argv[] = {"", 0};
    uint64_t fds[2] = {0};
    err = full_spawn(0, 1, argv, fds, pid);
    CHECK(err);

    // int exit_code = 0;
    uint8_t block[7] = {1, 2, 3, 4, 5, 6, 7};
    size_t actual_length = 0;
    err = read_exact(fds[CKB_STDIN], block, sizeof(block), &actual_length);
    CHECK(err);
    err = ckb_close(fds[CKB_STDIN]);
    CHECK(err);

    err = ckb_close(fds[CKB_STDOUT]);
exit:
    return err;
}

int child_write_closed_fd() {
    int err = 0;
    uint64_t inherited_fds[2];
    size_t inherited_fds_length = 2;
    err = ckb_inherited_file_descriptors(inherited_fds, &inherited_fds_length);
    CHECK(err);

    uint8_t block[7] = {0};
    size_t actual_length = 0;
    err = write_exact(inherited_fds[CKB_STDOUT], block, sizeof(block),
                      &actual_length);
    CHECK(err);
    err = write_exact(inherited_fds[CKB_STDOUT], block, sizeof(block),
                      &actual_length);
    CHECK(err);

    ckb_close(inherited_fds[CKB_STDIN]);
    ckb_close(inherited_fds[CKB_STDOUT]);

exit:
    return err;
}

int parent_pid(uint64_t* pid) {
    int err = 0;

    uint64_t cur_pid = ckb_process_id();

    uint64_t pid_c1 = 0;
    const char* argv[] = {"", 0};
    uint64_t fds_1[2] = {0};
    err = full_spawn(0, 1, argv, fds_1, &pid_c1);
    CHECK2(pid_c1 != cur_pid, -1);

    uint64_t pid_c2 = 0;
    uint64_t fds_2[2] = {0};
    err = full_spawn(0, 1, argv, fds_2, &pid_c2);
    CHECK(err);
    CHECK2(pid_c2 != cur_pid, -1);

    uint64_t child_pid_1 = 0;
    size_t actual_length = 0;
    err = read_exact(fds_1[CKB_STDIN], &child_pid_1, sizeof(child_pid_1),
                     &actual_length);
    CHECK(err);
    CHECK2(child_pid_1 == pid_c1, -1);

    uint64_t child_pid_2 = 0;
    err = read_exact(fds_2[CKB_STDIN], &child_pid_2, sizeof(child_pid_2),
                     &actual_length);
    CHECK(err);
    CHECK2(child_pid_2 == pid_c2, -1);

exit:
    return err;
}

int child_pid() {
    uint64_t pid = ckb_process_id();

    int err = 0;
    uint64_t fds[2] = {0};
    uint64_t fds_length = 2;
    err = ckb_inherited_file_descriptors(fds, &fds_length);
    CHECK(err);

    // send pid
    size_t actual_length = 0;
    err = write_exact(fds[CKB_STDOUT], &pid, sizeof(pid), &actual_length);
    CHECK(err);

exit:
    return err;
}

int parent_spawn_offset_out_of_bound(uint64_t* pid) {
    int err = 0;

    const char* argv[] = {"", 0};
    spawn_args_t spgs = {
        .argc = 1, .argv = argv, .process_id = pid, .inherited_fds = NULL};
    uint64_t offset = 1024 * 1024 * 1024 * 1;
    uint64_t length = 0;
    uint64_t bounds = (offset << 32) + length;
    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, bounds, &spgs);
    CHECK2(err == 3, -1);  // SLICE_OUT_OF_BOUND
    err = 0;
exit:
    return err;
}

int parent_spawn_length_out_of_bound(uint64_t* pid) {
    int err = 0;

    const char* argv[] = {"", 0};
    spawn_args_t spgs = {
        .argc = 1, .argv = argv, .process_id = pid, .inherited_fds = NULL};
    uint64_t offset = 1024 * 14;
    uint64_t length = 1024;
    uint64_t bounds = (offset << 32) + length;

    err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, bounds, &spgs);
    CHECK2(err == 3, -1);  // SLICE_OUT_OF_BOUND
    err = 0;
exit:
    return err;
}

int parent_entry(int case_id) {
    int err = 0;
    uint64_t pid = 0;
    if (case_id == 1) {
        err = parent_simple_read_write(&pid);
    } else if (case_id == 2) {
        err = parent_write_dead_lock(&pid);
    } else if (case_id == 3) {
        err = parent_invalid_fd(&pid);
    } else if (case_id == 4) {
        err = parent_wait_dead_lock(&pid);
    } else if (case_id == 5) {
        err = parent_read_write_with_close(&pid);
    } else if (case_id == 6) {
        err = parent_wait_multiple(&pid);
    } else if (case_id == 7) {
        err = parent_inherited_fds(&pid);
    } else if (case_id == 8) {
        err = parent_inherited_fds_without_owner(&pid);
    } else if (case_id == 9) {
        err = parent_read_then_close(&pid);
    } else if (case_id == 10) {
        err = parent_max_vms_count(&pid);
        return err;
    } else if (case_id == 11) {
        err = parent_max_pipe_limits(&pid);
        return err;
    } else if (case_id == 12) {
        return parent_close_invalid_fd(&pid);
    } else if (case_id == 13) {
        return parent_write_closed_fd(&pid);
    } else if (case_id == 14) {
        return parent_pid(&pid);
    } else if (case_id == 15) {
        return parent_spawn_offset_out_of_bound(&pid);
    } else if (case_id == 16) {
        return parent_spawn_length_out_of_bound(&pid);
    } else {
        CHECK2(false, -2);
    }
    CHECK(err);
    int8_t exit_code = 0;
    err = ckb_wait(pid, &exit_code);
    CHECK(err);
    CHECK(exit_code);

exit:
    return err;
}

int child_entry(int case_id) {
    if (case_id == 1) {
        return child_simple_read_write();
    } else if (case_id == 2) {
        return child_write_dead_lock();
    } else if (case_id == 3) {
        return 0;
    } else if (case_id == 4) {
        return child_wait_dead_lock();
    } else if (case_id == 5) {
        return child_read_write_with_close();
    } else if (case_id == 6) {
        return 0;
    } else if (case_id == 7) {
        return child_inherited_fds();
    } else if (case_id == 8) {
        return 0;
    } else if (case_id == 9) {
        return child_read_then_close();
    } else if (case_id == 10) {
        return child_max_vms_count();
    } else if (case_id == 11) {
        return child_max_pipe_limits();
    } else if (case_id == 12) {
        return 0;
    } else if (case_id == 13) {
        return child_write_closed_fd();
    } else if (case_id == 14) {
        return child_pid();
    } else if (case_id == 15) {
        return 0;
    } else if (case_id == 16) {
        return 0;
    } else {
        return -1;
    }
}

int main(int argc, const char* argv[]) {
    uint8_t script_args[8];
    size_t script_args_length = 8;
    int err = load_script_args(script_args, &script_args_length);
    if (err) {
        return err;
    }
    int case_id = (int)script_args[0];
    if (argc > 0) {
        return child_entry(case_id);
    } else {
        return parent_entry(case_id);
    }
}
