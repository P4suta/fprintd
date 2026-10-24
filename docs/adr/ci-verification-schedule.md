---
date: 2026-10-24
---

# CI verification schedule

## Status

Accepted.

[Affected-file verification](affected-file-verification.md) supersedes the summary's treatment of unaffected prerequisite jobs.

## Context

The required platform tests, native libfprint integration, minimum Rust version, package checks, dependency policy, and license checks provide the merge verdict.
Mutation annotations and coverage measurements provide advisory information without a required threshold.
Main-branch push verification repeats checks on commits that pass pull-request CI.

## Decision

Run the required CI workflow for pull requests and merge groups.
Keep the existing platform and package verification commands.
Require an explicit successful result from every predecessor in both summary jobs.
Run CodeQL for pull requests without main-push or scheduled repetition.
Keep mutation and coverage commands available for developer investigations without automatic hosted campaigns.
Remove scheduled verification against the upcoming Rust toolchain.

## Consequences

Routine changes use fewer hosted jobs and avoid duplicate main-branch verification.
The required gate rejects skipped, failed, cancelled, and missing verification results.
Mutation survivors and coverage reports require an explicit local invocation.
Compatibility with an upcoming Rust toolchain requires an explicit investigation rather than a recurring campaign.
