#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    const size_t length = 1000000;
    uint64_t *values = malloc(length * sizeof(*values));
    if (values == NULL) {
        return 1;
    }
    for (size_t index = 0; index < length; ++index) {
        values[index] = index + 1;
    }
    uint64_t total = 0;
    for (size_t index = 0; index < length; ++index) {
        total += values[index];
    }
    printf("%" PRIu64 "\n", total);
    free(values);
    return 0;
}
