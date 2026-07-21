<!--
Thanks for contributing! A few reminders (see CONTRIBUTING.md for the full loop):
- Commits follow Conventional Commits (feat: / fix: / perf: / docs: / …); PRs are squash-merged.
- Dependencies flow only toward the leaves; `fprint-core` and the NBIS kernels stay charter-clean.
- Do not hand-edit generated artifacts (NBIS goldens, CHANGELOG.md) — regenerate them.
-->

## What & why

Describe the change and the motivation. Link any related issue (`Closes #123`).

## Linear

Closes DEV-___
<!-- Links this PR to its Linear issue; requires the Linear GitHub integration. -->

## Checklist

- [ ] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass
- [ ] `cargo test --workspace --all-features` passes
- [ ] `mise run lint`, `mise run reuse`, and `mise run deny` pass
- [ ] New or changed product code is covered by a test that pins its behavior
      (the `mutants` CI gate mutation-tests the diff)
- [ ] NBIS goldens were regenerated with `mise run bozorth3-oracle` / `mise run mindtct-oracle`, not hand-edited
- [ ] Docs / ADRs updated if user-facing
