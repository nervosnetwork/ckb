#include "spawn_utils.h"

// 2.4 M bytes
static uint64_t g_data[300 * 1024];

int main() {
    int err = 0;
    uint64_t fds[2] = {0};
    uint64_t pid = 0;
    uint64_t current_pid = ckb_process_id();
    size_t argc = 1;
    const char* argv[2] = {"", 0};
    int8_t exit_code = 0;

    printf("current pid = %d", current_pid);
    for (size_t i = 0; i < sizeof(g_data) / sizeof(uint64_t); i++) {
        g_data[i] = current_pid;
    }

    if (current_pid == 7) {
        // wait forever
        ckb_wait(0, &exit_code);
    } else {
        err = full_spawn(0, argc, argv, fds, &pid);
        CHECK(err);
        if (current_pid == 0) {
            uint8_t buf[1] = {0};
            while (true) {
                size_t len = 1;
                ckb_read(fds[CKB_STDIN], buf, &len);
            }
        } else if (current_pid == 1) {
            uint64_t inherited_fds[3];
            size_t fds_len = 3;
            err = ckb_inherited_fds(inherited_fds, &fds_len);
            CHECK(err);
            uint8_t buf[1] = {0};
            while (true) {
                size_t len = 1;
                ckb_write(inherited_fds[CKB_STDOUT], buf, &len);
                ckb_read(fds[CKB_STDIN], buf, &len);
            }
        } else if (current_pid == 2) {
            uint64_t inherited_fds[3];
            size_t fds_len = 3;
            err = ckb_inherited_fds(inherited_fds, &fds_len);
            CHECK(err);
            uint8_t buf[1] = {0};
            while (true) {
                size_t len = 1;
                ckb_write(inherited_fds[CKB_STDOUT], buf, &len);
            }
        } else {
            // wait forever
            ckb_wait(0, &exit_code);
        }
    }
    // avoid g_data to be optimized
    for (size_t i = 0; i < sizeof(g_data) / sizeof(uint64_t); i++) {
        err += (int8_t)g_data[i];
    }

exit:
    return err;
}
