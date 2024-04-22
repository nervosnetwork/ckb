#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

const uint64_t SYSCALL_CYCLES_BASE = 500;
const uint64_t SPAWN_EXTRA_CYCLES_BASE = 100000;
const uint64_t SPAWN_YIELD_CYCLES_BASE = 800;

int tic() {
    static uint64_t tic = 0;
    uint64_t cur_cycles = ckb_current_cycles();
    uint64_t toc = cur_cycles - tic;
    tic = cur_cycles;
    return toc;
}

uint64_t cal_cycles(uint64_t nbase, uint64_t yield, uint64_t extra) {
    uint64_t r = 0;
    r += SYSCALL_CYCLES_BASE * nbase;
    r += SPAWN_YIELD_CYCLES_BASE * yield;
    r += SPAWN_EXTRA_CYCLES_BASE * extra;
    return r;
}

int main() {
    int err = 0;
    int toc = 0;
    uint64_t cid = ckb_process_id();
    uint64_t pid[5] = {0};
    uint64_t fds[5][2][3] = {0};
    uint64_t buf[256] = {0};
    uint64_t len = 0;

    switch (cid) {
        case 0:
            const char* argv[1] = {0};

            for (int i = 1; i < 5; i++) {
                toc = tic();
                err = ckb_pipe(buf);
                CHECK(err);
                toc = tic();
                CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
                fds[i][0][0] = buf[0];
                fds[i][1][1] = buf[1];

                toc = tic();
                err = ckb_pipe(buf);
                CHECK(err);
                toc = tic();
                CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
                fds[i][0][1] = buf[1];
                fds[i][1][0] = buf[0];
            }

            // Living Process: 0
            // Living Process: 0, 1
            // Living Process: 0, 1, 2
            // Living Process: 0, 1, 2, 3
            // Living Process: 1, 2, 3, 4
            // Living Process: 0, 2, 3, 4
            for (int i = 1; i < 5; i++) {
                toc = tic();
                spawn_args_t spgs = {.argc = 0, .argv = argv, .process_id = &pid[i], .inherited_fds = fds[i][1]};
                err = ckb_spawn(0, CKB_SOURCE_CELL_DEP, 0, 0, &spgs);
                CHECK(err);
                toc = tic();
                if (i < 5) {
                    CHECK2(toc > cal_cycles(1, 1, 1), ErrorCommon);
                } else {
                    CHECK2(toc > cal_cycles(1, 1, 4), ErrorCommon);
                }
            }

            // Living Process: 0, 2, 3, 4
            // Living Process: 0, 1, 3, 4
            // Living Process: 0, 2, 3, 4
            for (int i = 1; i < 5; i++) {
                len = 12;
                toc = tic();
                err = ckb_write(fds[i][0][1], "Hello World!", &len);
                toc = tic();
                CHECK(err);
                if (i < 3) {
                    CHECK2(toc > cal_cycles(1, 1, 2), ErrorCommon);
                } else {
                    CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
                }
                err = ckb_close(fds[i][0][1]);
                CHECK(err);
                toc = tic();
                CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
            }

            // Living Process: 0, 2, 3, 4
            // Living Process: 0, 1, 3, 4
            // Living Process: 0, 2, 3, 4
            // Living Process: 0, 3, 4
            // Living Process: 0, 4
            // Living Process: 0
            for (int i = 1; i < 5; i++) {
                len = 1024;
                toc = tic();
                err = ckb_read_all(fds[i][0][0], buf, &len);
                CHECK(err);
                toc = tic();
                if (i == 1) {
                    CHECK2(toc > cal_cycles(1, 1, 2), ErrorCommon);
                }
                if (i == 2) {
                    CHECK2(toc > cal_cycles(1, 1, 1), ErrorCommon);
                }
                if (i >= 3) {
                    CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
                }
                CHECK2(len == 12, ErrorCommon);
                err = memcmp("Hello World!", buf, len);
                CHECK(err);
            }

            for (int i = 1; i < 5; i++) {
                int8_t exit_code = 255;
                toc = tic();
                err = ckb_wait(pid[i], &exit_code);
                CHECK(err);
                toc = tic();
                CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
                CHECK(exit_code);
            }
            break;
        case 1:
        case 2:
        case 3:
        case 4:
            len = 2;
            toc = tic();
            err = ckb_inherited_file_descriptors(fds[cid][1], &len);
            CHECK(err);
            toc = tic();
            CHECK2(toc > cal_cycles(1, 1, 0), ErrorCommon);
            CHECK2(len == 2, ErrorCommon);
            len = 1024;
            err = ckb_read_all(fds[cid][1][0], buf, &len);
            CHECK(err);
            CHECK2(len == 12, ErrorCommon);
            err = memcmp("Hello World!", buf, len);
            CHECK(err);
            err = ckb_write(fds[cid][1][1], buf, &len);
            CHECK(err);
            err = ckb_close(fds[cid][1][1]);
            CHECK(err);
            break;
    }

exit:
    return err;
}
