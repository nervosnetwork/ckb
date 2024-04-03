#ifndef ESCAPE_ENCODING_H_
#define ESCAPE_ENCODING_H_

#include <stddef.h>
#include <stdint.h>

#ifndef ESCAPE_ERROR_ENCODING
#define ESCAPE_ERROR_ENCODING 1
#endif /* ESCAPE_ERROR_ENCODING */

size_t ee_maximum_encoding_length(size_t length) { return length * 2; }

int ee_decode(uint8_t *dst, size_t *dst_length, const uint8_t *src,
              size_t *src_length) {
  size_t ds = 0;
  size_t dl = *dst_length;
  size_t ss = 0;
  size_t sl = *src_length;

  while ((ss < sl) && (ds < dl)) {
    if (src[ss] == 0xFE) {
      if (ss + 1 >= sl) {
        return ESCAPE_ERROR_ENCODING;
      }
      dst[ds++] = src[ss + 1] + 1;
      ss += 2;
    } else {
      dst[ds++] = src[ss++];
    }
  }

  *dst_length = ds;
  *src_length = ss;
  return 0;
}

int ee_decode_char_string_in_place(char *buf, size_t *length) {
  size_t ss = 0;
  size_t ds = 0;

  while (buf[ss] != '\0') {
    if (((uint8_t)buf[ss]) == 0xFE) {
      if (buf[ss + 1] == '\0') {
        return ESCAPE_ERROR_ENCODING;
      }
      buf[ds++] = buf[ss + 1] + 1;
      ss += 2;
    } else {
      buf[ds++] = buf[ss++];
    }
  }

  *length = ds;
  return 0;
}

int ee_encode(uint8_t *dst, size_t *dst_length, const uint8_t *src,
              size_t *src_length) {
  size_t ds = 0;
  size_t dl = *dst_length;
  size_t ss = 0;
  size_t sl = *src_length;

  while ((ss < sl) && (ds < dl)) {
    if ((src[ss] == 0x0) || (src[ss] == 0xFE)) {
      if (ds + 1 >= dl) {
        break;
      }
      dst[ds] = 0xFE;
      dst[ds + 1] = src[ss++] - 1;
      ds += 2;
    } else {
      dst[ds++] = src[ss++];
    }
  }

  *dst_length = ds;
  *src_length = ss;
  return 0;
}

#endif /* ESCAPE_ENCODING_H_ */
