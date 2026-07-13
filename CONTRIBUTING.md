# Contributing to Pop Lang

Pop Lang is architecture-first. The documents under `architecture/` are the
project contract. A patch that merely compiles is not necessarily a patch that
belongs here.

## Before you start

Read:

1. [`architecture/README.md`](architecture/README.md);
2. [`architecture/19-architecture-conformance-and-regression-policy.md`](architecture/19-architecture-conformance-and-regression-policy.md);
3. the architecture document and accepted ADRs related to your change;
4. [`AGENTS.md`](AGENTS.md).

Search the repository with `rg` before changing terminology, examples, or a
cross-cutting contract. If the behavior is not authorized by the architecture,
write the design/ADR first. An issue, prototype, or existing implementation is
not permission to invent semantics.

## The expected workflow

1. Explain the problem and identify the authorizing architecture section or
   ADR.
2. For a semantic or public change, update the architecture and accepted ADR
   process before implementation.
3. Add deterministic tests for positive behavior, rejection boundaries,
   conventions, consistency, and relevant regressions. Tests come before the
   implementation.
4. Implement the smallest change that satisfies those tests.
5. Synchronize examples, terminology, diagnostics, and conformance matrices.
6. Run the relevant checks locally and report anything you could not run.

Keep changes narrow. Do not mix unrelated cleanup, formatting churn, or
dependency additions into a feature patch.

## AI-assisted work

AI tools and coding agents are legitimate contribution tools. Use them for
research, exploration, implementation, test writing, review, documentation,
or repetitive work when they help. Agent-produced contributions are evaluated
by their result, not by who or what wrote them. If a change follows the project
intent and architecture, has the required tests, and meets the quality bar, it
is welcome.

There is no special review penalty or disclosure requirement for using AI. The
normal project requirements still apply: the change must be coherent, must not
claim checks that were not run, and must not introduce private code,
credentials, or unlicensed material.

## Technical bar

- Preserve Pop Lang's Luau-shaped, low-ceremony syntax.
- Keep every runtime value and operation statically typed.
- Do not add dynamic values, implicit globals, runtime string lookup, broad
  reflection, universal tables, or dynamic fallback IR.
- Keep HIR and MIR backend-neutral.
- Preserve `Item → Module → Bubble → Package → Workspace` terminology.
- Use complete names and the repository's casing rules.
- Prefer data, functions, records, unions, namespaces, and composition before
  introducing classes or inheritance.
- Do not weaken or rewrite a failing test to make contradictory code pass.

Technical review is direct. Reviewers should identify concrete defects,
architectural drift, missing tests, and compatibility risks. Contributors are
expected to answer those points with code, tests, or evidence—not with appeals
to popularity or ownership.

## Local checks

Use the repository-pinned Rust 1.96 toolchain:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Run narrower checks while iterating, but run the full relevant set before
requesting review. Do not commit build output, dependency caches, credentials,
editor files, or generated artifacts.

## Commits and pull requests

Commit messages follow Linux Git conventions: a short, specific subject line
in the imperative mood, no trailing period, and a blank line before any longer
explanation. The body should explain why the change is needed and describe
important consequences that are not obvious from the code. Commit messages
should remain concise, readable, and focused on one logical change.

Pull requests should explain the problem, the architectural authorization, the
test coverage, and any remaining limitations. One logical change per pull
request is preferred whenever practical.

A pull request is not complete while it leaves contradictory old terminology,
uncovered public behavior, or an unexplained failing check. Maintainers may
request a smaller patch, a design change, or a test that proves the relevant
boundary.

## Avoid using Python

Repository scripts should not be written in Python. Ruby is preferred when a
script is needed and Ruby is available.

Contributors are responsible for installing Ruby on their own development
machines. If Ruby cannot be installed, do not add the script. Only commit
scripts that are necessary for the repository, and do not commit Python
scripts.
