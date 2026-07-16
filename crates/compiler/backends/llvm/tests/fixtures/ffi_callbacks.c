#include <stddef.h>
#include <stdint.h>

typedef int32_t (*PopCallback)(int32_t, void *);
typedef void *(*PopPointerCallback)(void *, void *);
typedef struct { int32_t left; int32_t right; } Pair;
typedef Pair (*PopRecordCallback)(Pair, void *);
typedef int64_t (*PopSystemCallback)(int64_t, void *);

int32_t visit_scalar_scoped(PopCallback callback, int32_t value, void *context) { return callback(value, context); }
int32_t visit_pointer_scoped(PopPointerCallback callback, void *context) { return callback(NULL, context) == NULL ? 23 : 24; }
int32_t visit_registered(PopCallback callback, int32_t value, void *context) { return callback(value, context); }
int32_t visit_record_scoped(PopRecordCallback callback, void *context) { Pair value = { 7, 9 }; Pair result = callback(value, context); return result.left == 7 && result.right == 9 ? 25 : 26; }
int32_t visit_system_scoped(PopSystemCallback callback, void *context) { return callback(11, context) == 11 ? 27 : 28; }

extern uint64_t pop_b10_sOPEN(int64_t);
extern uint64_t pop_b10_sUSE(uint64_t, int64_t);
extern uint64_t pop_b10_sCLOSE(uint64_t);
extern uint64_t pop_rt_attach_managed_thread(uint32_t);
extern uint8_t pop_rt_detach_managed_thread(uint64_t);
extern uint64_t pop_rt_field_get(uint64_t, uint64_t);
extern uint64_t pop_rt_enter_foreign(uint32_t, uint64_t *, uint64_t, uint8_t);
extern uint8_t pop_rt_leave_foreign(uint64_t, uint64_t *, uint64_t);

int main(void) {
    uint64_t binding = pop_rt_attach_managed_thread(1);
    if (binding == 0) return 10;
    uint64_t opened = pop_b10_sOPEN(42);
    if (pop_rt_field_get(opened, 1) != 0) return 11;
    uint64_t callback = pop_rt_field_get(opened, 2);
    if (callback == 0) return 12;
    uint64_t paired = pop_b10_sUSE(callback, 7);
    if (pop_rt_field_get(paired, 1) != 0 || pop_rt_field_get(paired, 2) != 42) return 13;
    uint64_t context = pop_rt_field_get(callback, 2);
    uintptr_t thunk_bits = (uintptr_t)pop_rt_field_get(callback, 4);
    if (context == 0 || thunk_bits == 0) return 14;
    uint64_t transition = pop_rt_enter_foreign(91, NULL, 0, 0);
    if (transition == 0) return 15;
    PopCallback thunk = (PopCallback)thunk_bits;
    int32_t returned = thunk(7, (void *)(uintptr_t)context);
    if (pop_rt_leave_foreign(transition, NULL, 0) != 1) return 16;
    uint64_t closed = pop_b10_sCLOSE(callback);
    if (pop_rt_field_get(closed, 1) != 0) return 17;
    uint64_t closed_pair = pop_b10_sUSE(callback, 7);
    if (pop_rt_field_get(closed_pair, 1) != 1) return 18;
    uint64_t closed_again = pop_b10_sCLOSE(callback);
    if (pop_rt_field_get(closed_again, 1) != 0) return 19;
    if (pop_rt_detach_managed_thread(binding) != 1) return 20;
    return returned == 42 ? 0 : 21;
}
