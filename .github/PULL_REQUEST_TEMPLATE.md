## Summary

<!-- What problem does this solve? Keep the scope explicit. -->

## Architecture traceability

- Authorizing architecture section or ADR:
- New or changed public contract:
- Architecture documents, examples, or terminology updated:

## Verification

- [ ] Tests were added or updated before implementation where behavior changed.
- [ ] Positive behavior is covered.
- [ ] Negative/rejection boundaries are covered.
- [ ] Convention, consistency, and regression coverage is present where relevant.
- [ ] Cross-backend or differential coverage is present where relevant.
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets`
- [ ] `cargo test --workspace --all-targets`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`

If a check was not run, explain why:

## Review notes

- [ ] No dynamic typing, runtime string lookup, broad reflection, or universal-table behavior was introduced.
- [ ] HIR/MIR remain backend-neutral.
- [ ] No generated artifacts, dependency caches, credentials, or editor files are included.
- [ ] This is ready for technical review.

<!-- Call out known limitations, architecture gaps, compatibility concerns, or
follow-up work. Do not hide them. -->

