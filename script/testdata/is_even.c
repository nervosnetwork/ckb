#include <stdbool.h>
#include <stdint.h>

__attribute__((visibility("default"))) bool is_even (uint64_t num) {
    if (num & 0x1) {
        return false;
    } else {
        return true;
    }
}
