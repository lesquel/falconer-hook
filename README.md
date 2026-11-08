# openmosh-hook

Host-side daemon and CLI for [OpenMosh](https://github.com/lesquel/OpenMosh). It watches your AI
agents (through Herdr or tmux), keeps an inbox of events, sends **end-to-end encrypted** push
notifications through ntfy/UnifiedPush, and executes approvals sent from your phone over SSH.

It opens **no permanent network port**. The only listener is the one-shot pairing listener of
`openmosh-hook pair`, bound to the Tailscale address for at most 5 minutes.

> Status: v0.1. Linux and macOS.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/lesquel/openmosh-hook/main/install.sh | sh
```

| Option | Effect |
|---|---|
| `--dry-run` | Print every step, change nothing |
| `--from-source` | `cargo install --git https://github.com/lesquel/openmosh-hook --locked` (the repository is private for now, so this uses your git credentials) |
| `--binary <path>` | Install a binary you already built |
| `--version <tag>` | Release to download (default: latest) |
| `--yes` | Accept the offers to install Herdr / Tailscale with their official installers |
| `--no-service` | Skip `openmosh-hook service install` |

The installer puts the binary in `~/.local/bin`, checks `sshd`, Herdr or tmux and Tailscale,
writes `~/.config/openmosh/config.toml` (`0600`), registers the user service and runs
`openmosh-hook doctor`. It prints each command before running it and never calls `sudo` itself.

## Commands

| Command | Purpose |
|---|---|
| `openmosh-hook serve` | Daemon (systemd user service / launchd agent): poll Herdr every second, record `blocked` → `approval_required` and `done` → `done`, publish encrypted pushes |
| `openmosh-hook emit --source <name> [--json]` | Record an event from an agent hook; the hook JSON comes on stdin. The pane is taken from `$HERDR_PANE_ID` (when `$HERDR_ENV=1`) or `$TMUX_PANE` |
| `openmosh-hook pair [--alias <name>] [--timeout <secs>] [--invert] [--uri]` | QR pairing (see below). `--invert` renders dark-on-light; `--uri` also prints the pairing URI, which carries the one-time token |
| `openmosh-hook pair-ssh --json` | Pairing over an existing SSH session; request `{"name","ppk","endpoint"}` on stdin, returns `{"device_id","alias","hpk"}` |
| `openmosh-hook sessions list --json` | Herdr and tmux sessions |
| `openmosh-hook agents list --json` | Agents with their state |
| `openmosh-hook inbox list --json [--since <seq>] [--limit <n>]` | Events with `seq > since` (default limit 100, max 500) |
| `openmosh-hook respond <event-id> --action allow\|deny\|text [--text <msg>] --json` | Deliver the answer into the agent's pane and push `resolved` to every device |
| `openmosh-hook devices list --json` | Paired devices |
| `openmosh-hook devices revoke <id> [--json]` | Remove the device's `authorized_keys` line and its record |
| `openmosh-hook devices set-endpoint <id> <url> [--json]` | Rotate the push endpoint (clears `endpoint_stale`) |
| `openmosh-hook integration install\|uninstall claude [--settings <path>] [--json]` | Add/remove Claude Code `Notification` and `Stop` hooks calling `emit --source claude` (default `~/.claude/settings.json`) |
| `openmosh-hook doctor [--json]` | Check Tailscale, Herdr/tmux, permissions, sshd host key, devices, keymaps, service |
| `openmosh-hook service install\|uninstall [--dry-run] [--json]` | Register/remove the user service |
| `openmosh-hook version [--json]` | Version and wire protocol versions |

Ids are b64url and may start with `-`; positional id arguments accept that.

### JSON envelope

With `--json` every command prints exactly one line:

```json
{"ok":true,"data":{…}}
{"ok":false,"error":{"code":"not_found","message":"event not found"}}
```

Exit code is `0` on success and `1` on error. Messages never contain secrets. Error codes:
`invalid_input`, `not_found`, `already_resolved`, `text_too_long`, `no_pane`, `no_keymap`,
`runtime_unavailable`, `runtime_error`, `invalid_endpoint`, `permissions_too_open`,
`config_error`, `io_error`, `store_error`, `registry_corrupt`, `storage_error`,
`tailscale_missing`, `tailscale_not_running`, `tailscale_tagged`, `tailscale_error`, `ssh_host_key_missing`,
`listen_failed`, `pair_closed`, `pair_cancelled`, `settings_invalid`, `service_error`, `internal`.

## Pairing

**QR (primary).** `openmosh-hook pair` refuses to run unless Tailscale is up and this host is a
node owned by your user (a tagged node is refused with `tailscale_tagged`). It binds a listener
to the host's Tailscale IPv4 on an ephemeral port and shows a small QR (`openmosh://pair?v=3&i=…&k=…&p=…&t=…`)
with the host's Tailscale IPv4, the listener port, its X25519 public key and a 16-byte one-time
token valid for 5 minutes. The alias, SSH user, SSH port and SSH host key fingerprint go back to
the phone inside the encrypted reply. For every
connection it checks, in order: the peer is in `100.64.0.0/10`, `tailscale whois` reports the same
login as this host and the peer is not a tagged node, the request decrypts, the token matches, it
has not expired or been used, the SSH key is a valid `ssh-ed25519` key and the push endpoint is
acceptable. The whole request must arrive within 10 s (or the time left before expiry, if less).
Device names lose control, invisible formatting (bidi, zero-width) and line-separator characters
and are capped at 64 characters. Every failure gets the same `pair_rejected` answer; after 5
failures from a peer that passed the network checks (other addresses and other users' nodes do
not count), on timeout or on Ctrl+C the token is burned and the command exits 1. On success the phone's key is appended to `~/.ssh/authorized_keys` restricted to
the tailnet (the file is left `0600` and `~/.ssh` `0700`; a symlinked file is followed, a dangling
symlink is refused):

```text
from="100.64.0.0/10,fd7a:115c:a1e0::/48",no-agent-forwarding,no-X11-forwarding ssh-ed25519 AAAA… openmosh:<device_id>
```

**SSH (secondary).** `pair-ssh --json` registers push keys for a phone that already reaches the
host over SSH with its own key; no key is added.

Push endpoints must be `https://`; `http://` is accepted only for a `100.64.0.0/10` address or a
`*.ts.net` name. URLs with credentials, and loopback (`127.0.0.0/8`, `::1`, `localhost`),
link-local, unspecified, multicast or broadcast hosts are refused. Pushes ignore proxy environment
variables, send `User-Agent: openmosh`, never follow redirects, and `404`/`410` mark the device
`endpoint_stale`.

`respond` claims the event atomically before sending anything, so of two concurrent answers only
one reaches the agent; the other gets `already_resolved`. `integration install` run from a new
executable path repoints the existing hooks at it.

## Files

| Path | Mode | Content |
|---|---|---|
| `$XDG_CONFIG_HOME/openmosh/config.toml` | `0600` | `host_alias`, `retention_days` (7), `poll_interval_ms` (1000), `sources`, `ssh_port`, `ssh_host_key_pub`, `[push] server`, `[bins]` |
| `$XDG_STATE_HOME/openmosh/` | `0700` | state directory |
| `…/events.db` | `0600` | SQLite inbox |
| `…/devices.toml` | `0600` | paired devices |
| `…/host.key` | `0600` | host X25519 private key |

Directories default to `~/.config` and `~/.local/state`. The hook refuses to run when any of
these is readable by group or others.

## Agent keymaps

Keystrokes per agent live in [`data/keymaps/`](./data/keymaps) and are embedded at build time.
`claude`: allow = `enter`, deny = `esc`, text is typed then `enter`. **These still need field
verification** against the Claude Code version in use; `doctor` warns until a keymap is marked
`verified = true`.

## Shared test vectors

[`tests/vectors/push_v1.json`](./tests/vectors/push_v1.json) and
[`tests/vectors/pair_v1.json`](./tests/vectors/pair_v1.json) are produced from fixed keys and nonces
and checked on every `cargo test`; the app's crypto tests consume them. All binary fields are
b64url without padding; string fields are the exact UTF-8 sent on the wire.

- `push_v1`: `host_priv, host_pub, device_priv, device_pub, device_id, nonce, key, plaintext, body`.
- `pair_v1`: `host_priv, host_pub, eph_priv, eph_pub, token, k_req, k_resp, nonce_req, req_plaintext, req_line, nonce_resp, resp_plaintext, resp_line`.
- The pairing HKDF salt is the **raw 16 token bytes** (the QR carries them b64url-encoded).
- `req_line` and `resp_line` are shown without their trailing `\n`; on the wire each line ends with one.

Regenerate (only when the wire format version changes):

```sh
OPENMOSH_WRITE_VECTORS=1 cargo test regenerate_vectors -- --ignored
```

## Architecture

Clean Architecture by feature. See [`ARCHITECTURE.md`](./ARCHITECTURE.md).

## Development

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo test --test cli -- --ignored   # QR pairing end to end over this machine's Tailscale IPv4
```

CLI tests run the binary against a temporary HOME with fake `herdr`, `tmux`, `tailscale` and
`systemctl` scripts; no test sends keys to a real pane or touches real user files.

## License

Proposed Apache-2.0 (pending confirmation).
