#include "spawn_utils.h"

#define MAX_MEMORY (4 * 1024 * 1024)
#define PAGE_SIZE (4 * 1024)

extern char _end[];

void dirty_all_pages() {
    uint64_t addr = (uint64_t)_end;
    while (addr < MAX_MEMORY) {
        uint8_t* ptr = (uint8_t*)addr;
        *ptr = 0;
        addr += PAGE_SIZE;
    }
}

int main(int argc, const char* argv[]) {
    int err = 0;
    if (argc > 0) {
        // child
        dirty_all_pages();
        uint64_t inherited_fds[2];
        size_t inherited_fds_length = 2;
        err = ckb_inherited_fds(inherited_fds, &inherited_fds_length);
        uint64_t length = MAX_MEMORY;
        // Write a piece of data starting from address 0 with a size of 4M.
        // It should not consume any memory.
        err = ckb_write(inherited_fds[CKB_STDOUT], 0, &length);
        // should be blocked forever since there is no reading on other end
        CHECK(err);
    } else {
        // parent
        for (size_t i = 0; i < 15; i++) {
            uint64_t pid = 0;
            const char* argv[] = {"", 0};
            uint64_t fds[2] = {0};
            err = full_spawn(0, 1, argv, fds, &pid);
            CHECK(err);
        }
        dirty_all_pages();
    }

exit:
    return err;
}
