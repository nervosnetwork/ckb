
#include "utils.h"

int main() {
    int err = 0;
    for (size_t i = 0; i < 10000; i++) {
        err = simple_spawn(0);
        CHECK(err);
    }

exit:
    return err;
}
