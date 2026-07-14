# Pop.Standard bootstrap reference metadata

This versioned metadata is the initial verified prelude type, function, and
compiler-attribute surface used while bootstrapping `Pop.Standard`. Entries
carry stable semantic IDs, type/signature/attachment contracts, and trusted
prelude status. Compiler roles are assigned only to identities loaded from
these verified files; a user declaration cannot gain a compiler role by
copying `CompileTime`, `Prelude`, or another trusted spelling.

ADR 0058's `api-baseline.tsv` is loaded with fixed total, entry-count, and
per-entry limits. Identities use canonical decimal spelling, namespace paths
must be rooted at `Pop`, prelude membership must agree with the `prelude` tier,
and documentation authorities must be normalized paths below `architecture/`.
The loader rejects a disagreement before the baseline can affect resolution.
