#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    uint64_t total = 0;
    for (uint64_t index = 1; index <= 20000; ++index) {
        uint64_t *values = malloc(256 * sizeof(*values));
        if (values == NULL) return 1;
        for (size_t slot = 0; slot < 256; ++slot) values[slot] = index;
        total += values[0];
        free(values);
    }
    printf("%" PRIu64 "\n", total);
    return 0;
}
