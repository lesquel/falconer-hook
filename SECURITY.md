# Security policy

## Supported versions

| Version | Supported |
|---|---|
| 0.2.x | Yes |
| < 0.2 | No: upgrade to the latest 0.2.x release |

Release candidates (`vX.Y.Z-rc.N`) are for testing and get no separate fixes.

## Reporting a vulnerability

Report it privately through GitHub's private vulnerability reporting:
<https://github.com/lesquel/falconer-hook/security/advisories/new>
(or the **Security** tab, then **Report a vulnerability**).

Please do not open a public issue, pull request or discussion for a vulnerability. Include:

- the falconer-hook version (`falconer-hook version`) and platform;
- what an attacker needs (network position, local account, a stolen phone, …);
- steps to reproduce, and what happens versus what you expected.

Remove secrets, private keys, real IP addresses and hostnames from logs before sending them.

## What to expect

- An acknowledgement within 7 days.
- An assessment (accepted or not, and why) within 14 days.
- For accepted reports, a fix in a new release, coordinated with you before the advisory is
  published. You are credited in the advisory unless you prefer otherwise.

Falconer is maintained by one person, so these are goals rather than guarantees.

## Threat model

falconer-hook runs on a computer you own and lets a phone you paired read agent events and answer
them. These are the assumptions and the deliberate trade-offs.

### The paired phone has a full shell

Pairing appends the phone's key to `~/.ssh/authorized_keys` as:

```text
restrict,pty,from="100.88.77.66,fd7a:115c:a1e0::1:2" ssh-ed25519 AAAA… openmosh:<device_id>
```

- `restrict` turns off agent, X11 and port forwarding and `~/.ssh/rc`.
- `pty` keeps an interactive terminal.
- `from=` pins the key to the phone's own tailnet addresses.
- There is **no `command=`** on purpose: Falconer is a terminal, and the app runs commands and
  opens shells on the host.

So **a stolen phone key is equivalent to access to your account from that phone's tailnet
address.** If a phone is lost or compromised, revoke it from the computer:

```sh
falconer-hook devices revoke <device-id>
```

This removes its `authorized_keys` line and device record, ends its live SSH sessions and prints
a checklist of what else to rotate. Also remove the device from your Tailscale network.

(`openmosh:` is a legacy identifier from the project's former name. It is kept so that phones
paired before the rename keep working.)

### Network exposure

- No permanent listening port. `falconer-hook pair` opens a one-shot listener on the Tailscale
  IPv4 for at most 5 minutes, accepts only peers of the same Tailscale user (tagged nodes are
  refused), and requires a one-time token and a `y/N` confirmation on the host.
- The phone reaches the host through OpenSSH over Tailscale. Securing sshd is up to you;
  `falconer-hook harden-ssh` turns off password logins, and `doctor` warns about them.

### Approvals

- An approval can be answered for **15 minutes**; after that it gets `expired`. One dated more
  than 60 seconds in the future (the host clock stepped back) is refused the same way.
- Right before sending a key, the hook checks that the pane is still the same agent showing the
  same prompt. Otherwise the answer is refused as `stale`.
- An event is claimed atomically, so of two concurrent answers only one reaches the agent.

### Push notifications

- ntfy/UnifiedPush pushes are **end-to-end encrypted** per device with XChaCha20-Poly1305, under a
  key derived with X25519 and HKDF. The ntfy server only sees ciphertext, padded to fixed sizes.
- Web Push payloads are encrypted as RFC 8291 requires and carry only a generic title, because
  browsers show them on the lock screen.
- Push endpoints that point at loopback, private or link-local addresses are refused, including
  after DNS resolution.

### Local state

- Config and state files are `0600` and their directories `0700`. The hook refuses to run when
  they are readable by group or others.
- External programs run with an argv, never through a shell.

### Releases

- Releases are signed with minisign, key id `59E37F1EF15B67E2`. The public key is in
  [`minisign.pub`](./minisign.pub).
- `install.sh` verifies the signature on `SHA256SUMS` and requires the trusted comment
  `falconer-hook <tag>`. It then checks the archive against `SHA256SUMS`.
- `install.sh` itself is not signed. To review it first, see
  [Verify before running](./README.md#verify-before-running).

### Out of scope

- An attacker who already has your user account or root on the host.
- A compromised Tailscale account or coordination server.
- Vulnerabilities in the agents themselves, in Herdr, tmux, OpenSSH or Tailscale. Please report
  those upstream.
