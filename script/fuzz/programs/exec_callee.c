#include <string.h>

int main(int argc, char* argv[]) {
    int s = 0;
    for (int i = 0; i < argc; i++) {
        s += strlen(argv[i]);
    }
    if (s % 256 != 0) {
        return 0;
    }
    return s;
}
