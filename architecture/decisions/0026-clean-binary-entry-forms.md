# ADR 0026: Clean Binary Entry Forms

- Status: accepted
- Date: 2026-07-11
- Supersedes: ADR 0025 exact entry-shape requirement
- Clarified by: ADR 0029 default internal visibility and empty results

## Context

ADR 0025 correctly restricted process entry selection to binary Bubbles and
removed signature scanning, but its one mandatory
`private function main(arguments: Array<String>): Int` shape makes a simple
print-only program carry unused arguments and an artificial `return 0`.
Libraries already need no entry item at all. Binary roots need equally clear
common-case syntax without weakening namespace visibility generally.

## Decision

A binary Bubble resolves exactly one namespace-scope function named `main`.
The entry accepts either no parameters or exactly one `Array<String>` parameter,
and returns either no value or exactly one `Int` process status. These forms are
therefore valid:

```luau
function main()
    print(42)
end
```

```luau
private function main()
    print(42)
end
```

```luau
private function main(arguments: Array<String>): Int
    return 0
end
```

Omitted visibility is a narrow binary-entry exception. The compiler assigns
`private` visibility only to the exact namespace-scope declaration named
`main` in a binary root. Under ADR 0029, other declarations that accept
visibility default to `internal`, and a library `main` is an ordinary function
that defaults to `internal`. Explicit `public` or `internal` visibility is
invalid for a binary entry.

A no-result `main` returns process status zero after its body completes. A
result-bearing `main` returns its `Int` as process status. A no-parameter entry
does not request or decode platform arguments; an argument-taking entry uses
ADR 0025's exact UTF-8 `Array<String>` adapter. Ordinary functions retain their
declared result types and never acquire process-status behavior.

Entry selection still occurs by the resolved `SymbolId`. The backend receives
the selected entry identity and shape; it does not search source names or MIR
signatures. Library, test, example, and benchmark Bubble entry policies remain
distinct target-kind contracts. In particular, a library Bubble never requires
`main` merely because the same Package also contains a binary Bubble.

This decision supersedes only ADR 0025's exact visibility, parameter, and result
shape. Its binary-only selection, argument preservation, and process-boundary
rules remain accepted.

## Consequences

- A print-only executable can use `function main()` with no artificial return.
- Programs request process arguments only when their entry declares them.
- Libraries and ordinary Modules retain the ADR 0029 default-internal rule and
  do not gain an entry requirement.
- The parser may recognize the shorthand before target selection, but semantic
  publication must reject it unless the declaration is the selected binary
  root entry.

## Required conformance tests

- `function main()` and `private function main()` execute and return status zero
  without an explicit `return`;
- no-argument `main(): Int` and argument-taking `main` with either no result or
  `Int` execute with the correct process status;
- the argument adapter is called only for the argument-taking forms;
- `public main`, `internal main`, extra parameters, other parameter types, and
  non-`Int` results are rejected;
- omitted binary-root `main` remains private while omitted ordinary
  declarations, including library `main`, resolve to internal;
- a Package containing `src/lib.pop` and `src/main.pop` compiles the library
  without an entry and applies the shorthand only to the binary root;
- MIR interpreter and LLVM execution agree for every accepted logical entry
  shape.

## Alternatives considered

### Require `return 0` for every executable

Rejected because successful fallthrough is unambiguous at the process boundary
and the ceremony obscures print-only and side-effecting programs.

### Make all namespace functions implicitly private

Rejected because it weakens the accepted visibility contract and makes public
API accidents harder to review.

### Give every library an optional process entry

Rejected because a library Bubble is not an executable. A Package may contain
both kinds without merging their compilation or visibility boundaries.

## Documents/components affected

Syntax and nomenclature, CLI/tooling and code units, closed decisions, parser
entry recovery, target-aware front-end validation, driver entry selection,
LLVM process wrappers, examples, and cross-backend conformance tests.
