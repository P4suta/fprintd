---
date: 2026-10-25
---

# Affected-file verification

## Status

Accepted.

## Context

Root documentation and repository metadata leave the compiled product inputs unchanged.
The platform, package, and source-analysis jobs repeat the same work for those changes.
[The execution schedule](ci-verification-schedule.md) removes duplicate and advisory campaigns but requires all platform jobs for every change.

## Decision

Run repository rules, their task tests, workflow lint, and license checks for every pull request and merge group.
Use the maintained path-filter action with explicit exclusions for known root documentation, documentation-tree files, generated root licenses, and root repository and license metadata.
Read changed paths from Git rather than the size-limited pull-request file API.
Treat every other path as an input to the runtime checks.
Run the existing platform, package, dependency, and CodeQL jobs when runtime inputs change.
Keep the selected jobs in one workflow so the summary observes every result.
Require successful selection and license jobs in both cases.
Selected jobs must succeed, and unselected jobs must report a skip.
Missing selectors, unknown values, failed jobs, cancelled jobs, and selected jobs that skip refuse the required verdict.

## Consequences

Documentation changes keep meaningful repository checks without compiling the product on every supported OS.
Runtime and workflow changes retain the existing verification commands.
Unknown new file locations select the full verification path.
GitHub event commit identities, Git differences, and path-filter matching remain external semantic boundaries.
