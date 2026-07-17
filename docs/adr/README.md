# Architecture Decision Records

One record per architectural decision. Each states the context, the decision, and its
consequences as they stand now; superseding a decision adds a new record and marks the old one
`Superseded`. `ARCHITECTURE.md` links here from its Key decisions table.

Format: numbered file, a `Status` (`Proposed` / `Accepted` / `Superseded`), and a date.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-dispatch-native-async-fn.md) | Dispatch: native `async fn` in trait, static, in the core | Accepted |
| [0002](0002-composite-backend.md) | Runtime backend heterogeneity: `CompositeBackend`, above the core | Accepted |
| [0003](0003-fprintd-not-libfprint.md) | fprintd compatibility, not libfprint compatibility | Accepted |
| [0004](0004-coexistence-shim-first.md) | Coexistence: what we install, what we borrow | Accepted |
| [0005](0005-provenance-licensing.md) | Provenance & licensing boundary | Accepted |
