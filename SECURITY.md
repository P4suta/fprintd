# Security Policy

This stack sits on an authentication path: it stores fingerprint templates and
speaks the D-Bus contract a PAM login trusts. The pure crates are
`#![forbid(unsafe_code)]` and the arithmetic kernels are verified bit-exact
against stock NBIS, but a logic flaw here is an authentication flaw — so we take
security reports seriously.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through GitHub's [private vulnerability
reporting](https://github.com/P4suta/fprintd/security/advisories/new)
(Security → Advisories → *Report a vulnerability*). Include:

- the affected crate and version / commit,
- the platform and how the daemon was configured, and
- a minimal reproducer and the observed impact.

We aim to acknowledge a report within a few days and will keep you updated as we
investigate. Once a fix is available we will coordinate a disclosure timeline
with you and credit you unless you prefer otherwise.

## Scope

In scope:

- **Template handling and storage** — the FP3 codec (`fprint-fp3`) parsing untrusted
  or attacker-influenced template bytes, and the `/var/lib/fprint` layout the
  daemon writes (permissions, path handling, cross-user leakage).
- **The D-Bus surface** (`fprintd`) — PolicyKit action enforcement, client
  isolation, and any way to bypass or confuse a verify/identify result.
- **The FFI boundary** in the shim (`fprint-backend-libfprint`) and `unsafe` in any
  transport leaf.
- **The arithmetic kernels** (`fprint-bozorth3`, `fprint-mindtct`) — panics, unbounded
  memory/CPU, or match scores that diverge from NBIS in a way affecting security.

Out of scope:

- The **experimental USB capture seam** (`fprint-backend-native`'s `usb` module). Its
  protocol values are unverified placeholders and it cannot talk to real hardware;
  it is a worked example, not a shipping driver.
- Weaknesses inherent to fingerprint biometrics as an authentication factor
  (presentation attacks, sensor spoofing) rather than to this implementation.

## Supported versions

Pre-1.0: only the latest `main` receives security fixes.

| Version | Supported |
| ------- | --------- |
| `main` (latest) | ✅ |
| older commits   | ❌ |

## Status note

The shim daemon has so far been exercised only against libfprint's *virtual*
drivers in Docker; it has not been hardened or reviewed against a real sensor or
a real PAM login. Treat current revisions accordingly.
