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
openmosh-hook setup        # config, service, agent hooks, doctor
openmosh-hook pair         # scan the QR code with OpenMosh
```

The installer downloads the release binary for your platform (`openmosh-hook-<target>.tar.gz`
for `x86_64`/`aarch64` Linux (static musl) and macOS) and checks it against the release's
`SHA256SUMS`. When [minisign](https://jedisct1.github.io/minisign/) or `rsign` (rsign2) is
installed, it first verifies `SHA256SUMS.minisig` with the pinned release key and aborts on
failure; without either it warns that only the checksum is checked (pass `--require-signature`
to abort instead). **While the repository is private**, anonymous downloads fail: install the
[GitHub CLI](https://cli.github.com), run `gh auth login`, and the installer uses
`gh release download` instead (set `OPENMOSH_NO_GH=1` to never use it). From a clone you can
also run `sh install.sh --from-source` or `--binary <path>`.

| Option | Effect |
|---|---|
| `--dry-run` | Print every step, change nothing |
| `--from-source` | `cargo install --git https://github.com/lesquel/openmosh-hook --locked` (the repository is private for now, so this uses your git credentials) |
| `--binary <path>` | Install a binary you already built |
| `--version <tag>` | Release to download (default: latest), with `gh` or curl/wget |
| `--yes` | Accept the offers to install Herdr / Tailscale with their official installers |
| `--no-service` | Skip `openmosh-hook service install` |
| `--require-signature` | Abort unless `SHA256SUMS.minisig` can be verified (needs `minisign` or `rsign`); release downloads only |

The installer puts the binary in `~/.local/bin`, checks `sshd`, Herdr or tmux and Tailscale,
writes `~/.config/openmosh/config.toml` (`0600`), registers the user service and runs
`openmosh-hook doctor`. It prints each command before running it and never calls `sudo` itself.

### Verifying a release by hand

Releases are signed with minisign. The public key is [`minisign.pub`](./minisign.pub)
(key id `59E37F1EF15B67E2`):

```text
RWTiZ1vxHn/jWfqAEjobeqloI9cL9pOA0/in+ayyCgPqT0gYDRKR1Ox5
```

Download `openmosh-hook-<target>.tar.gz`, `SHA256SUMS` and `SHA256SUMS.minisig` from the
release, then:

```sh
minisign -Vm SHA256SUMS -P RWTiZ1vxHn/jWfqAEjobeqloI9cL9pOA0/in+ayyCgPqT0gYDRKR1Ox5
# or: rsign verify -P RWTiZ1vxHn/jWfqAEjobeqloI9cL9pOA0/in+ayyCgPqT0gYDRKR1Ox5 -x SHA256SUMS.minisig SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS     # macOS: shasum -a 256 --ignore-missing -c SHA256SUMS
```

### Setup and uninstall

`openmosh-hook setup [--yes] [--no-service] [--json]` writes the default config if there is
none, installs the user service, offers the integration of every [detected agent](#agents)
(asks on a terminal; `--yes` installs them all; with `--json` and no `--yes` it only lists
them), runs `doctor` and prints the next step (`openmosh-hook pair`). Running it again is safe.

`openmosh-hook uninstall [--all] [--dry-run] [--yes] [--json]` removes everything
openmosh-hook created, and nothing else:

1. the user service (stopped and disabled first),
2. every agent integration it installed (only its own hooks; your other hooks stay),
3. the `authorized_keys` lines tagged `openmosh:` (paired phones; your own keys stay),
4. `$XDG_CONFIG_HOME/openmosh` and `$XDG_STATE_HOME/openmosh` (config, devices, events, host key),
5. with `--all`, the `openmosh-hook` binary that is running.

Without `--yes` it prints the plan and asks on a terminal; without a terminal (or with `--json`)
it refuses with `confirmation_required`. `--dry-run` prints the plan and changes nothing.

## TUI

`openmosh-hook` with no command on a terminal (or `openmosh-hook tui`) opens a terminal UI over
the same commands as the CLI. Without a terminal, no command prints the help as before.

| Screen | What it does |
|---|---|
| 1 Status | `doctor` (config, Tailscale, Herdr/tmux, permissions, sshd, devices, service) and the version; `r` refreshes |
| 2 Agents & inbox | Agents with their state and the inbox, newest first; `a` allow, `d` deny, `t` reply with text to the selected pending event, each after a `y` confirmation |
| 3 Devices | Paired devices, `(endpoint stale)` marks; `x` revokes after confirmation |
| 4 Pair | `Enter` suspends the TUI and runs `openmosh-hook pair` in the terminal (`i` toggles `--invert`); Enter returns |
| 5 Integrations | Every supported agent, detected or not, installed or not; `i` installs, `u` uninstalls after confirmation |
| 6 Setup | `s` runs `setup`, `a` runs `setup --yes` (also installs every detected agent's integration), after confirmation |
| 7 Uninstall | Shows the `uninstall --dry-run` plan; `a` toggles `--all`; `Enter` asks you to type `uninstall` before anything is removed |

`Tab`/`Shift+Tab` or `1`-`7` switch screens, `j`/`k` or the arrows select, `q` or `Esc` quits.
The terminal is restored on exit and on a panic. After a complete uninstall the TUI runs nothing
else (it would recreate state); quit with `q`.

## Commands

| Command | Purpose |
|---|---|
| `openmosh-hook serve` | Daemon (systemd user service / launchd agent): poll Herdr every second, record `blocked` → `approval_required` and `done` → `done`, publish encrypted pushes |
| `openmosh-hook emit --source <name> [--kind <kind>] [--json]` | Record an event from an agent hook; the hook JSON comes on stdin. The pane is taken from `$HERDR_PANE_ID` (when `$HERDR_ENV=1`) or `$TMUX_PANE`. `--kind approval_required\|question\|done\|notification` fixes the kind for agents whose payload does not say it; the payload is then optional (only its message is kept, and a terminal on stdin is not read). `notification` becomes `approval_required` when the payload is about a permission, else `question` |
| `openmosh-hook pair [--alias <name>] [--timeout <secs>] [--invert] [--uri]` | QR pairing (see below). `--invert` renders dark-on-light; `--uri` also prints the pairing URI, which carries the one-time token |
| `openmosh-hook pair-ssh --json` | Pairing over an existing SSH session; request `{"name","ppk","endpoint"}` on stdin, returns `{"device_id","alias","hpk"}` |
| `openmosh-hook sessions list --json` | Herdr and tmux sessions |
| `openmosh-hook agents list --json` | Agents with their state |
| `openmosh-hook inbox list --json [--since <seq>] [--limit <n>]` | Events with `seq > since` (default limit 100, max 500) |
| `openmosh-hook respond <event-id> --action allow\|deny\|text [--text <msg>] --json` | Deliver the answer into the agent's pane and push `resolved` to every device. Text answers are sent as one line (newlines become spaces, control characters are removed), then Enter |
| `openmosh-hook devices list --json` | Paired devices |
| `openmosh-hook devices revoke <id> [--json]` | Remove the device's `authorized_keys` line and its record |
| `openmosh-hook devices set-endpoint <id> <url> [--json]` | Rotate the push endpoint (clears `endpoint_stale`) |
| `openmosh-hook integration install\|uninstall <agent> [--settings <path>] [--json]` | Add/remove the [agent's hooks](#agents) calling `emit --source <agent>`; `--settings` overrides the default file |
| `openmosh-hook integration list [--json]` | Every supported agent: detected on this machine (binary on `PATH` or config dir present), integration installed, file, docs link |
| `openmosh-hook setup [--yes] [--no-service] [--json]` | First-time [setup](#setup-and-uninstall) |
| `openmosh-hook uninstall [--all] [--dry-run] [--yes] [--json]` | Remove everything openmosh-hook created ([details](#setup-and-uninstall)) |
| `openmosh-hook doctor [--json]` | Check Tailscale, Herdr/tmux, permissions, sshd host key, sshd password logins (`sshd -G`; warns when password or keyboard-interactive login is on), devices, keymaps, service |
| `openmosh-hook harden-ssh [--dry-run] [--yes] [--json]` | Make sshd accept SSH keys only: writes `/etc/ssh/sshd_config.d/10-openmosh.conf` with `sudo`, validates it with `sshd -t` (removed again on failure) and reloads sshd. Asks before changing anything; also `s` on the TUI Status screen |
| `openmosh-hook service install\|uninstall [--dry-run] [--json]` | Register/remove the user service |
| `openmosh-hook attach herdr [--session <name>]` | Run the Herdr client (`herdr` or `herdr session attach <name>`, from `[bins] herdr`) with [focus mirroring](#mirroring); exits with Herdr's status code |
| `openmosh-hook tui` | The [terminal UI](#tui) (also `openmosh-hook` alone on a terminal) |
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
from="100.64.0.0/10,fd7a:115c:a1e0::/48",no-agent-forwarding,no-X11-forwarding,no-port-forwarding,no-user-rc ssh-ed25519 AAAA… openmosh:<device_id>
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

## Mirroring

Herdr keeps a separate view per attached client, so navigating on the phone does not move the
computer. `openmosh-hook attach herdr` runs the real Herdr client on the inherited terminal and,
next to it, subscribes to Herdr's `workspace.focused`, `tab.focused` and `pane.focused` events on
the local socket. When any client focuses a new target, the mirror sends `workspace.focus`,
`tab.focus` or `pane.focus`, which Herdr applies to every attached client. Herdr 0.9 does not
emit `tab.focused` when a client switches tabs, so the mirror also reads `session.snapshot` every
250 ms (its focus follows the client that navigated last) and treats a changed workspace or tab
the same way.

- **Both directions.** Events come from every client, so the computer follows the phone and the
  phone follows the computer. One mirror is enough; several running at once converge.
- **No loops.** The last applied target per kind is remembered and its echo is ignored. Changes of
  the same kind are rate-limited to one per 150 ms; the latest one inside the window wins.
- **Recovery.** On `events_lost` or a dropped socket it reconnects and takes the current focus from
  `session.snapshot` as the new baseline.
- **Best effort.** Without a reachable socket Herdr still runs, and one warning is printed when it
  exits. The socket is `--session`'s, then `$HERDR_SOCKET_PATH`, then `$HERDR_SESSION`'s, then
  `$XDG_CONFIG_HOME/herdr/herdr.sock`.

Limits: pane sizes follow the last active client (Herdr resizes to it), and an explicit focus
marks a `done` agent in that pane as seen.

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

## Agents

`integration install <agent>` wires each agent's own, documented hook mechanism. Every edit keeps
your other settings and hooks, is idempotent, repoints our commands when the binary moved, and
refuses to touch a file it cannot parse (or, for files it owns, one it did not write).
`integration uninstall` removes only what it added.

| Agent | File edited | Events → kind | Docs |
|---|---|---|---|
| `claude` Claude Code | `~/.claude/settings.json` | `Notification`, `Stop` (payload parsed) | [hooks](https://code.claude.com/docs/en/hooks) |
| `codex` OpenAI Codex CLI | `~/.codex/hooks.json` | `PermissionRequest` → approval, `Stop` → done | [hooks](https://learn.chatgpt.com/docs/hooks) |
| `gemini` Gemini CLI | `~/.gemini/settings.json` | `Notification` → notification, `AfterAgent` → done | [hooks](https://geminicli.com/docs/hooks/) |
| `cursor` Cursor | `~/.cursor/hooks.json` | `stop` → done | [hooks](https://cursor.com/docs/agent/hooks) |
| `copilot` GitHub Copilot CLI | `~/.copilot/hooks/openmosh.json` (own file) | `notification` → notification, `agentStop` → done | [hooks](https://docs.github.com/en/copilot/reference/hooks-reference) |
| `kiro` Kiro CLI | `~/.kiro/hooks/openmosh.json` (own file) | `Stop` → done | [hooks](https://kiro.dev/docs/hooks/) |
| `opencode` OpenCode | `$XDG_CONFIG_HOME/opencode/plugins/openmosh.js` (own plugin) | `permission.asked` → approval, `question.asked` → question, `session.idle` → done | [plugins](https://opencode.ai/docs/plugins/) |
| `aider` Aider | `~/.aider.conf.yml` (marked block) | `notifications_command` → done | [notifications](https://aider.chat/docs/usage/notifications.html) |

Notes: Codex asks you to review and trust new hooks on its next start; Copilot CLI, Kiro CLI and
OpenCode load hooks/plugins when they start. The OpenCode plugin runs `openmosh-hook` with an
argv (no shell). The Aider block refuses to install when you already set `notifications` or
`notifications_command` yourself. `CODEX_HOME` and `COPILOT_HOME` are not followed: pass
`--settings` when you use them. **Amp** documents no hook or notification command (only
sounds), so it has no integration; Herdr state polling still reports it.

### Keymaps

Keystrokes per agent live in [`data/keymaps/`](./data/keymaps) and are embedded at build time:
`claude`, `codex`, `gemini`, `cursor`, `copilot`, `opencode`, `amp` (allow `enter`, deny `esc`;
`codex` allows with `y`) and `kiro`, `aider` (`y`/`n` then `enter`). Text answers are typed, then
`enter`. **All of them still need field verification** against the agent version in use;
`doctor` warns until a keymap is marked `verified = true`.

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

CLI tests (`tests/cli.rs`, `tests/agents.rs`, `tests/lifecycle.rs`, sharing `tests/common/`) run
the binary against a temporary HOME with fake `herdr`, `tmux`, `tailscale` and `systemctl`
scripts; no test sends keys to a real pane or touches real user files. `tests/install_sh.rs`
runs `install.sh --dry-run` with fake `uname`/`gh`/`minisign`/`rsign`.

### Releases

Pushing a `v*` tag runs [`.github/workflows/release.yml`](./.github/workflows/release.yml): it
builds `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` (with `cross`),
`x86_64-apple-darwin` and `aarch64-apple-darwin`, and publishes
`openmosh-hook-<target>.tar.gz` plus one `SHA256SUMS` and its minisign signature
`SHA256SUMS.minisig` to the GitHub release. The signing key is the repository secret
`MINISIGN_SECRET_KEY` (an unencrypted minisign secret key file); the job writes it to
`$RUNNER_TEMP` with umask `077`, deletes it right after signing, and checks the signature
against the committed [`minisign.pub`](./minisign.pub), so a mismatched secret fails the release.

## License

Copyright 2026 lesquel. Licensed under the [Apache License 2.0](./LICENSE). Redistributions
must keep the [NOTICE](./NOTICE) file, which credits the author.
