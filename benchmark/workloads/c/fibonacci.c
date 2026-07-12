#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>

static uint64_t fibonacci(uint64_t value) {
    if (value < 2) {
        return value;
    }
    return fibonacci(value - 1) + fibonacci(value - 2);
}

int main(void) {
    uint64_t total = 0;
    for (uint64_t index = 0; index < 30; ++index) {
        total += fibonacci(28);
    }
    printf("%" PRIu64 "\n", total);
    return 0;
}
