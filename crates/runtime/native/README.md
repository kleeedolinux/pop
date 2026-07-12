# Native Runtime Facade

`pop-runtime-native` composes the portable collector with the versioned native
ABI. It owns exported C functions, the process-global synchronized bootstrap
instance, UTF-8 and process-entry adaptation, and native trap/unwind
termination.

Heap storage, reachability, roots, pins, and collection policy remain in
`pop-runtime-collector`; symbol/version vocabulary remains in
`pop-runtime-native-abi`. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).

The facade is divided into `identity`, `allocation`, `storage`, `text`, `roots`,
`failure`, and private `state` modules. This keeps ABI exports grouped by the
runtime service they adapt while retaining one static library and one native ABI.
