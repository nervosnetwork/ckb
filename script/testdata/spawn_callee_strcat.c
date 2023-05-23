#include <stdint.h>
#include <string.h>

#include "ckb_syscalls.h"

int main(int argc, char *argv[]) {
  char content[80];
  for (int i = 0; i < argc; i++) {
    strcat(content, argv[i]);
  }
  uint64_t content_size = (uint64_t)strlen(content);
  ckb_set_content((uint8_t *)&content[0], &content_size);
  if (content_size != (uint64_t)strlen(content)) {
    return 1;
  }
  return 0;
}
