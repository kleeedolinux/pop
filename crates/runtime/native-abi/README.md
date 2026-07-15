# Native Runtime ABI

`pop-runtime-native-abi` owns the closed, versioned C vocabulary used by the
native backend and trusted native bootstrap adapters. It maps accepted PLRI
operations to constant `pop_rt_*` symbols and records physical sentinel rules.
ABI 1.11 includes `pop_rt_allocate_initialized_object`, whose exact map and
initializer arrays represent one failure-atomic object publication.
ABI 1.13 adds `pop_rt_enter_foreign` and `pop_rt_leave_foreign` as distinct,
balanced transition entries with writable exact root arrays. ABI 1.12 remains
the immutable task-frame descriptor and both earlier descriptors stay
supported.

[ADR 0078](../../../architecture/decisions/0078-native-abi-2-writable-root-coexistence.md)
adds distinct immutable ABI 1.11 and ABI 2.0 descriptors. ABI 2 owns the
separate `pop_rt_gc_safe_point_v2` writable-root spelling and the fixed
`pop_rt_supports_abi` negotiation spelling; their presence never makes an
incomplete facade advertise ABI 2 support.

It owns no heap, collector, exported function implementation, process-global
state, or backend lowering. Unsupported operations return no symbol instead of
receiving a fallback. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).
