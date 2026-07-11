# Pop Lang Milestone 0 syntax boundary

This crate begins the pipeline authorized by the implementation roadmap and
ADR 0005. Its first vertical slice recognizes:

- one file-scoped `namespace` header;
- semicolon-free `using` headers;
- `public`, `internal`, and `private` namespace declarations;
- functions, constants, type aliases, attributes, records, unions, classes,
  interfaces, and enums at their structural block/line boundary;
- typed function-signature wrappers for generic parameters, qualified and
  generic names, arrays, typed tables, tuples, optionals, unions, and function
  values;
- Luau-shaped `function`/`end` blocks;
- ordinary `--` comments and attachable `---` documentation trivia;
- lossless tokens for the accepted punctuation and declaration keywords.

This is an implementation milestone, not a complete language grammar. Tokens
that are preserved before their grammar milestone do not acquire runtime or
semantic behavior. Resolution, type checking, HIR, and MIR remain in their
separate crates and later roadmap milestones.
