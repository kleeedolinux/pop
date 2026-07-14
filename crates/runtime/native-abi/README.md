# Native Runtime ABI

`pop-runtime-native-abi` owns the closed, versioned C vocabulary used by the
native backend and trusted native bootstrap adapters. It maps accepted PLRI
operations to constant `pop_rt_*` symbols and records physical sentinel rules.
ABI 1.11 includes `pop_rt_allocate_initialized_object`, whose exact map and
initializer arrays represent one failure-atomic object publication.

It owns no heap, collector, exported function implementation, process-global
state, or backend lowering. Unsupported operations return no symbol instead of
receiving a fallback. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).
