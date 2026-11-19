#include "ckb_raw_syscalls.h"

// The integration test appends its private genesis header to this ELF. Check it
// against the first header dependency before emitting any observations.
static int matches_test_genesis(void) {
    uint8_t header[256], expected[sizeof(header)];
    uint64_t size = sizeof(header), code_size = 0;
    if (ckb_load_header(header, &size, 0, 0, CKB_SOURCE_HEADER_DEP) != 0 || size == 0 || size > sizeof(header) ||
        ckb_load_cell_data(0, &code_size, 0, 0, CKB_SOURCE_CELL_DEP) != 0 || code_size < size) {
        return 0;
    }
    uint64_t tail_size = size;
    if (ckb_load_cell_data(expected, &tail_size, code_size - size, 0, CKB_SOURCE_CELL_DEP) != 0 || tail_size != size) {
        return 0;
    }
    for (uint64_t i = 0; i < size; ++i) {
        if (header[i] != expected[i]) return 0;
    }
    return 1;
}

static void observe(void) {
    static const char digits[] = "0123456789abcdef";
    static const char prefix[] = "Freezer extension ";
    uint8_t data[32];
    uint64_t size = sizeof(data);
    if (ckb_load_block_extension(data, &size, 0, 1, CKB_SOURCE_HEADER_DEP) != 0 || size != sizeof(data)) {
        ckb_debug("Freezer extension unavailable or unexpected length");
        return;
    }

    char line[sizeof(prefix) + 2 * sizeof(data)];
    for (unsigned i = 0; i < sizeof(prefix) - 1; ++i) line[i] = prefix[i];
    for (unsigned i = 0; i < sizeof(data); ++i) {
        line[sizeof(prefix) - 1 + 2 * i] = digits[data[i] >> 4];
        line[sizeof(prefix) + 2 * i] = digits[data[i] & 15];
    }
    line[sizeof(line) - 1] = 0;
    ckb_debug(line);
}

void _start(void) {
    if (matches_test_genesis()) observe();
    // The host compares each observation with the known bytes.
    ckb_exit(0);
    __builtin_unreachable();
}
