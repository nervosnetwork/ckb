/* Real VM consumer for the archived-store fixture. No C library is required. */
typedef unsigned long u64;

static u64 extension(void* buffer, u64* length, u64 offset, u64 index, u64 source) {
    register u64 a0 __asm__("a0") = (u64)buffer;
    register u64 a1 __asm__("a1") = (u64)length;
    register u64 a2 __asm__("a2") = offset;
    register u64 a3 __asm__("a3") = index;
    register u64 a4 __asm__("a4") = source;
    register u64 a7 __asm__("a7") = 2104;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7) : "memory");
    return a0;
}

static int check(void) {
    unsigned char bytes[128];
    const u64 sources[] = {1, 3, 4, 0x0100000000000001UL};
    for (u64 i = 0; i < 4; i++) {
        u64 length = sizeof(bytes);
        if (extension(bytes, &length, 0, 0, sources[i]) != 0 || length != 96) return 1;
        for (u64 j = 0; j < length; j++)
            if (bytes[j] != 0x7f) return 2;
        length = 8;
        if (extension(bytes, &length, 11, 0, sources[i]) != 0 || length != 85) return 3;
        for (u64 j = 0; j < 8; j++)
            if (bytes[j] != 0x7f) return 4;
        length = 0;
        if (extension(bytes, &length, 0, 0, sources[i]) != 0 || length != 96) return 5;
        if (extension(bytes, &length, 0, 99, sources[i]) != 1) return 6;
    }
    u64 length = sizeof(bytes);
    /* Legitimately absent extension and a different hash at the same height. */
    if (extension(bytes, &length, 0, 1, 4) != 2) return 7;
    if (extension(bytes, &length, 0, 2, 4) != 0 || length != 17) return 8;
    for (u64 j = 0; j < length; j++)
        if (bytes[j] != 0xa5) return 9;
    if (extension(bytes, &length, 0, 0, 2) != 1) return 10;
    return 0;
}

void _start(void) {
    register u64 a0 __asm__("a0") = check();
    register u64 a7 __asm__("a7") = 93;
    __asm__ volatile("ecall" : : "r"(a0), "r"(a7) : "memory");
    __builtin_unreachable();
}
