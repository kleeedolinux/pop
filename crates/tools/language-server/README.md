# Pop Lang language server

This crate contains the private incremental engine for the official Pop Lang
language server. It is not the public `Pop.Lsp` Package.

The implemented bootstrap slice owns:

- one immutable language and localization context per server session;
- full-text, versioned open-document snapshots;
- stable session-local `FileId` values;
- rejection of duplicate opens and stale changes;
- cancellation before a snapshot or analysis result is published;
- compiler diagnostics with stable `POP####` codes;
- checked-documentation hover and document symbols; and
- conversion from UTF-8 source offsets to LSP UTF-16 positions.

The `pop-language-server` executable exposes that engine through a bounded LSP
3.17 JSON-RPC stdio adapter. It advertises full-text document synchronization,
compiler diagnostics, checked-documentation hover, and document symbols. The
initialization `locale`
selects presentation first; when absent, `POP_LANGUAGE`, tool configuration,
the system locale, and English follow ADR 0088 precedence.

The current crate does not implement completion, signature help, cross-Module
navigation, references, rename, formatting, semantic tokens, code actions,
incremental text edits, Workspace analysis, or public syntax values. Those
surfaces depend on reviewed schemas in the independently
installed `Pop.Rpc`, `Pop.Syntax`, and `Pop.Lsp` Packages. The private compiler
syntax tree and query handles must not be exported as a shortcut.
