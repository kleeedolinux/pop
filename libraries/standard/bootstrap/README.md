# Pop.Standard bootstrap reference metadata

This versioned metadata is the initial verified prelude type and compiler-
attribute surface used while bootstrapping `Pop.Standard`. Entries carry stable
semantic IDs, type/attachment contracts, and trusted prelude status. Compiler
roles are assigned only to identities loaded from these verified files; a user
declaration cannot gain a compiler role by copying `CompileTime`, `Prelude`, or
another trusted spelling.
