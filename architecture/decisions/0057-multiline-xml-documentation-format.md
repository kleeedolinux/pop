# ADR 0057: Multiline XML Documentation Format

- Status: accepted
- Date: 2026-07-14
- Supersedes: the inline-short-element formatting rule in architecture section 20

## Context

Pop Lang documentation comments use checked XML carried by Lua-shaped `---`
lines. The original formatting contract kept a short `<summary>` on one line.
That form becomes difficult to scan once a declaration also documents type
parameters, parameters, results, errors, effects, allocation, and examples. It
also gives the formatter a subjective line-length threshold for choosing between
two source shapes.

Documentation is a public contract and standard-library source contains many
adjacent tags. One deterministic shape is preferable to retaining a compact
exception that agents, examples, fixtures, and tools reproduce inconsistently.

## Decision

Every non-empty XML documentation element uses separate documentation lines for
its opening tag, content, and closing tag:

```luau
--- <summary>
--- Describes division failures.
--- </summary>
public error DivideError
end
```

Sibling top-level contract elements are separated by one empty documentation
line. The empty line is spelled `---` without trailing whitespace:

```luau
--- <summary>
--- Divides one integer by another.
--- </summary>
---
--- <param name="value">
--- The dividend.
--- </param>
---
--- <returns>
--- The integer quotient.
--- </returns>
public function divide(value: Int, divisor: Int): Int
    return value / divisor
end
```

This rule applies to summaries, type parameters, parameters, results, errors,
panic/effect/cost contracts, remarks, examples, links with body text, custom
elements, and every other non-empty documentation element. Nested XML remains
nested within the surrounding element, but its opening and closing tags do not
share a documentation line with body text.

A genuinely empty element may retain XML self-closing syntax such as
`--- <inheritdoc/>`. The formatter does not rewrite malformed or unsafe XML
destructively. `<code>` content retains its accepted whitespace semantics.

The canonical formatter converts a well-formed one-line element to the
multiline form, inserts the required empty documentation line between adjacent
top-level contract elements, and is deterministic and idempotent. Parsing and
semantic documentation identity do not change: formatting changes source shape,
not the checked XML tree, public API, runtime metadata, HIR, MIR, or ABI.

Repository architecture examples, Pop source, fixtures, and agent guidance use
only this canonical form. Tests may construct malformed documentation to verify
diagnostics, but no valid non-empty XML documentation element remains inline.

## Consequences

- Documentation blocks are longer but have one predictable, reviewable shape.
- Diffs add or remove complete tag/content blocks without line-length-driven
  reflow.
- Agents no longer decide whether an element is short enough for inline form.
- The formatter must understand documentation tokens and preserve indentation.
- Existing valid inline documentation is reformatted repository-wide.

## Alternatives considered

### Keep short summaries inline

Rejected because it preserves two canonical shapes and a subjective readability
threshold.

### Keep parameters and results inline but expand summaries

Rejected because signature contracts would still alternate between incompatible
shapes and remain dense in standard-library source.

### Change only examples without formatter enforcement

Rejected because new inline comments would immediately reappear and tools would
have no executable canonical contract.

## Required conformance tests

- convert every well-formed non-empty inline documentation element;
- preserve declaration indentation on every generated `---` line;
- insert exactly one empty documentation line between sibling contract elements;
- preserve self-closing, malformed, unsafe, and ordinary comments;
- preserve nested body markup and `<code>` whitespace;
- prove deterministic idempotence; and
- reject repository regressions through a scan for valid inline documentation.

## Documents/components affected

XML documentation architecture, syntax/nomenclature, formatter, documentation
tests, standard-library source, compiler fixtures, architecture examples, and
agent skills.
