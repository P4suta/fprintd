# 0004 — Coexistence: what we install, what we borrow

- Status: Accepted
- Date: 2026-07-18

## Context

Two fingerprint daemons cannot run at once: one sensor, one `/var/lib/fprint`, one owner of the
`net.reactivated.Fprint` D-Bus name.

## Decision

The daemon ships one file: the systemd unit (`crates/fprintd/dbus/`). The D-Bus policy, the
PolicyKit actions, and `pam_fprintd.so` come from the fprintd package, a hard dependency
(`Depends:`, not `Recommends:`). The package is `fprintd-rs` and the binary installs as
`/usr/libexec/fprintd-rs`, so both can be installed at once; only the bus name is shared.
Taking the seat is `systemctl enable fprintd-rs`, whose `Alias=fprintd.service` shadows the
upstream unit from `/etc/systemd/system` so D-Bus activation reaches us; `disable` reverts it.

## Consequences

The dependency is all-or-nothing. Without the D-Bus policy nothing may own
`net.reactivated.Fprint`, not even root, so the daemon does not start; without the PolicyKit
actions every privileged method is denied. This settles three points:

- **No own PAM module.** It would not remove the dependency — the policy and actions still come
  from upstream — and would add 26 KB to the authentication path. `pam_fprintd` is a D-Bus
  client and works while the contract holds.
- **The D-Bus contract is not extended.** The borrowed policy allowlists exactly
  `net.reactivated.Fprint.Manager`, `.Device`, and the three standard interfaces, so a method
  on an interface of our own would never reach us.
- **No `Conflicts=fprintd.service`.** Under the alias, that name is our own unit.

Known gap: SELinux and AppArmor label by path, and those labels ship with the distro's policy
package, not with fprintd, so they cannot be borrowed. `/usr/libexec/fprintd-rs` does not
transition into `fprintd_t` and may be denied `/var/lib/fprint`. The fix is for
`/usr/libexec/fprintd` to become an `update-alternatives` link upstream — a proposal to make
there.
