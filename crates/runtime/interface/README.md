# PLRI Contract

`pop-runtime-interface` owns backend-neutral semantic runtime values,
operations, precise maps, failures, and adapter traits. It is the dependency
leaf shared by MIR, backends, collectors, and trusted runtime adapters.

This crate must not contain native C symbol spellings, exported functions,
platform types, process-global runtime state, collector storage, or compiler
backend implementation types. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).

The foreign boundary is represented by closed `ForeignCallMode` and
single-use `ForeignTransitionId` values plus distinct `EnterForeign` and
`LeaveForeign` operations. Physical token allocation, thread binding, and C
symbol spellings remain runtime-adapter concerns under
[ADR 0081](../../../architecture/decisions/0081-statically-bound-native-ffi.md).
Native entry authority is separately represented by the nonzero,
single-use `ManagedThreadBindingId` and balanced `AttachManagedThread`/
`DetachManagedThread` operations.
