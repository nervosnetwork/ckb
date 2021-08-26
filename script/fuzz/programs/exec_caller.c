#include <stdint.h>

static inline long __internal_syscall(long n, long _a0, long _a1, long _a2,
                                      long _a3, long _a4, long _a5) {
  register long a0 asm("a0") = _a0;
  register long a1 asm("a1") = _a1;
  register long a2 asm("a2") = _a2;
  register long a3 asm("a3") = _a3;
  register long a4 asm("a4") = _a4;
  register long a5 asm("a5") = _a5;

#ifdef __riscv_32e
  register long syscall_id asm("t0") = n;
#else
  register long syscall_id asm("a7") = n;
#endif

  asm volatile("scall"
               : "+r"(a0)
               : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(syscall_id));
  return a0;
}

#define syscall(n, a, b, c, d, e, f)                                           \
  __internal_syscall(n, (long)(a), (long)(b), (long)(c), (long)(d), (long)(e), \
                     (long)(f))

uint64_t get_u64(uint8_t *buf) {
  return ((uint64_t)buf[0] << 0x00) + ((uint64_t)buf[1] << 0x08) +
         ((uint64_t)buf[2] << 0x10) + ((uint64_t)buf[3] << 0x18) +
         ((uint64_t)buf[4] << 0x20) + ((uint64_t)buf[5] << 0x28) +
         ((uint64_t)buf[6] << 0x30) + ((uint64_t)buf[7] << 0x38);
}

int main() {
  uint8_t buf[262144] = {};
  uint64_t len = 262144;
  if (syscall(2092, buf, &len, 0, 2, 3, 0) != 0) {
    return 1;
  }

  uint64_t p = 0;
  uint8_t callee_from = buf[p];
  p += 1;

  uint64_t callee_offset = buf[p];
  p += 1;

  uint64_t callee_length = get_u64(&buf[p]);
  p += 8;

  uint64_t argc = get_u64(&buf[p]);
  p += 8;

  char *argv[262144] = {};
  for (int i = 0; i < argc; i++) {
    uint64_t l = get_u64(&buf[p]);
    p += 8;
    argv[i] = &buf[p];
  }

  if (callee_from == 0) {
    // Callee from dep cell
    syscall(2043, 1, 3, 0, (callee_offset << 32) | callee_length, argc, argv);
  } else if (callee_from == 1) {
    // Callee from witness input
    syscall(2043, 0, 1, 1, (callee_offset << 32) | callee_length, argc, argv);
  } else if (callee_from == 2) {
    // Callee from witness output
    syscall(2043, 0, 2, 1, (callee_offset << 32) | callee_length, argc, argv);
  } else {
    return 1;
  }
  return 1;
}
