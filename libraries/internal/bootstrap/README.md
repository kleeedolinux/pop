# Pop.Internal bootstrap schemas

These versioned, deterministic TSV files are the minimal compiler-owned input
used to bootstrap and verify `Pop.Internal` as required by the base-library
architecture. They are not a user extension point and do not make intrinsic
recognition depend on source spelling alone.

`primitives.tsv` maps accepted source names and aliases to canonical runtime
roles. `types.tsv` assigns stable identities and arities to foundational nominal
and collection types. `intrinsics.tsv` assigns stable intrinsic identities,
typed signatures, backend-neutral lowering kinds, and required target
capabilities. The type component parses and cross-validates every file before
it can be consumed by later resolution, HIR, or MIR stages.
