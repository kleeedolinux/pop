#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>

int main(void) {
    uint64_t value = 0;
    for (uint64_t index = 1; index <= UINT64_C(50000000); ++index) {
        value += index;
    }
    printf("%" PRIu64 "\n", value);
    return 0;
}
