#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

struct item {
    uint64_t value;
};

int main(void) {
    const size_t count = 200000;
    struct item **values = malloc(count * sizeof(*values));
    if (values == NULL) return 1;
    for (size_t index = 0; index < count; ++index) {
        values[index] = malloc(sizeof(*values[index]));
        if (values[index] == NULL) return 1;
        values[index]->value = index + 1;
    }
    uint64_t total = 0;
    for (size_t index = 0; index < count; ++index) {
        total += values[index]->value;
        free(values[index]);
    }
    free(values);
    printf("%" PRIu64 "\n", total);
    return 0;
}
