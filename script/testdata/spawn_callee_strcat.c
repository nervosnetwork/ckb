#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"
#include "spawn_utils.h"

char *strcat(char *restrict dest, const char *restrict src) {
    strcpy(dest + strlen(dest), src);
    return dest;
}

int main(int argc, char *argv[]) {
    int err = 0;
    char content[80];
    for (int i = 0; i < argc; i++) {
        strcat(content, argv[i]);
    }
    size_t content_size = (uint64_t)strlen(content);
    uint64_t fds[2] = {0};
    uint64_t length = countof(fds);
    err = ckb_inherited_file_descriptors(fds, &length);
    CHECK(err);
    CHECK2(length == 2, ErrorCommon);
    size_t content_size2 = content_size;
    printf("fds[CKB_STDOUT] = %d", fds[CKB_STDOUT]);
    err = ckb_write(fds[CKB_STDOUT], content, &content_size);
    CHECK(err);
    CHECK2(content_size2 == content_size, ErrorWrite);

exit:
    return err;
}
