# openmosh-hook

Host-side daemon and CLI for [OpenMosh](https://github.com/lesquel/OpenMosh). It watches your AI
agents (through Herdr or tmux), keeps an inbox of events, sends **end-to-end encrypted** push
notifications through ntfy/UnifiedPush, and executes approvals sent from your phone over SSH.

It opens **no permanent network port**. The only listener is the one-shot pairing listener of
`openmosh-hook pair`, bound to the Tailscale address for at most 5 minutes.

> Status: v0.2.0. Linux (x86_64, aarch64) and macOS (Intel, Apple Silicon).

Supported agents: Claude Code, Codex CLI, Gemini CLI, Cursor, GitHub Copilot CLI, Kiro CLI,
OpenCode and Aider through their own hooks ([Agents](#agents)); Amp through Herdr state polling
only. Highlights of v0.2.0: the [TUI](#tui), a `y/N` confirmation on the host for every
[pairing](#pairing), pairing without notifications, [`harden-ssh`](#commands), stale-approval
checks in `respond`, revoke that ends the phone's live sessions, [`usage`](#usage),
[mirroring](#mirroring) with `attach herdr`, and signed releases required by the installer.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/lesquel/openmosh-hook/main/install.sh | sh
openmosh-hook setup        # config, service, agent hooks, doctor
openmosh-hook pair         # scan the QR code with OpenMosh
```

The installer downloads the release binary for your platform (`openmosh-hook-<target>.tar.gz`
for `x86_64`/`aarch64` Linux (static musl) and macOS) and checks it against the release's
`SHA256SUMS`, after verifying `SHA256SUMS.minisig` with the pinned release key. The signature
is required: install [minisign](https://jedisct1.github.io/minisign/) (`apt`/`dnf`/`pacman`/
`brew install minisign`) or `rsign` (`cargo install rsign2 --locked`) first; without either the
installer stops before downloading and says how to get one (`--insecure-no-signature` checks the
checksums only, not recommended). The signature's trusted comment must be exactly
`openmosh-hook <tag>` for the release being installed (`latest` is resolved to its tag first),
so an older or different release cannot be served in its place. **While the repository is private**, anonymous downloads fail: install the
[GitHub CLI](https://cli.github.com), run `gh auth login`, and the installer uses
`gh release download` instead (set `OPENMOSH_NO_GH=1` to never use it). From a clone you can
also run `sh install.sh --from-source` or `--binary <path>`.

| Option | Effect |
|---|---|
| `--dry-run` | Print every step, change nothing |
| `--from-source` | `cargo install --git https://github.com/lesquel/openmosh-hook --locked`, with `--tag <tag>` when `--version` is given (else the default branch); the repository is private for now, so this uses your git credentials |
| `--binary <path>` | Install a binary you already built |
| `--version <tag>` | Release to download, `vX.Y.Z` or a pre-release `vX.Y.Z-rc.N` (default: the latest stable release), with `gh` or curl/wget |
| `--yes` | Accept the offers to install Herdr / Tailscale with their official installers |
| `--no-service` | Skip `openmosh-hook service install` |
| `--insecure-no-signature` | Skip the signature check (checksums only); prints a warning. Not recommended |
| `--require-signature` | Accepted for compatibility: the signature is always required |

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

Check that the trusted comment minisign prints is `openmosh-hook <the tag you downloaded>`.

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

The sshd drop-in written by `harden-ssh` (`/etc/ssh/sshd_config.d/10-openmosh.conf`) needs root
to remove and removing it turns password logins back on, so it is kept and listed in the plan
(`kept` in `--json`) with the command to remove it yourself.

Without `--yes` it prints the plan and asks on a terminal; without a terminal (or with `--json`)
it refuses with `confirmation_required`. `--dry-run` prints the plan and changes nothing.

## TUI

`openmosh-hook` with no command on a terminal (or `openmosh-hook tui`) opens a terminal UI over
the same commands as the CLI. Without a terminal, no command prints the help as before.

| Screen | What it does |
|---|---|
| 1 Status | `doctor` (config, Tailscale, Herdr/tmux, permissions, sshd, devices, service) and the version; `r` refreshes |
| 2 Agents & inbox | Agents with their state and the inbox, newest first; `a` allow, `d` deny, `t` reply with text to the selected pending event, each after a `y` confirmation |
| 3 Devices | Paired devices, `(endpoint stale)` / `(no notifications)` marks; `x` revokes after confirmation |
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
| `openmosh-hook serve` | Daemon (systemd user service / launchd agent): poll Herdr every second while an agent works or waits (every 5 s when all are idle), record `blocked` → `approval_required` and `done` → `done`, publish encrypted pushes |
| `openmosh-hook emit --source <name> [--kind <kind>] [--json]` | Record an event from an agent hook; the hook JSON comes on stdin. The pane is taken from `$HERDR_PANE_ID` (when `$HERDR_ENV=1`) or `$TMUX_PANE`. `--kind approval_required\|question\|done\|notification` fixes the kind for agents whose payload does not say it; the payload is then optional (only its message is kept, and a terminal on stdin is not read). `notification` becomes `approval_required` when the payload is about a permission, else `question` |
| `openmosh-hook pair [--alias <name>] [--timeout <secs>] [--invert] [--uri] [--yes]` | QR pairing (see below). Asks `Pair "<name>" from <node> (<ip>)? [y/N]` before authorizing the phone; `--yes` skips that (required without a terminal). `--invert` renders dark-on-light; `--uri` also prints the pairing URI, which carries the one-time token |
| `openmosh-hook pair-ssh --json` | Pairing over an existing SSH session; request `{"name","ppk","endpoint"}` on stdin, returns `{"device_id","alias","hpk"}` |
| `openmosh-hook sessions list --json` | Herdr and tmux sessions |
| `openmosh-hook agents list --json` | Agents with their state |
| `openmosh-hook inbox list --json [--since <seq>] [--limit <n>]` | Events with `seq > since` (default limit 100, max 500) |
| `openmosh-hook respond <event-id> --action allow\|deny\|text [--text <msg>] --json` | Deliver the answer into the agent's pane and push `resolved` to every device. Text answers are sent as one line (newlines become spaces, control characters are removed), then Enter |
| `openmosh-hook devices list --json` | Paired devices |
| `openmosh-hook devices revoke <id> [--json]` | Remove the device's `authorized_keys` line and its record, end its live SSH sessions (this user's sessions whose `SSH_CONNECTION` peer is one of the phone's pinned addresses, found in `/proc`; never the session running the command; devices paired before addresses were pinned cannot be told apart and get `sessions_ended: null`), and print a lost-phone checklist (`checklist` in `--json`) |
| `openmosh-hook devices set-endpoint <id> <url> [--json]` | Rotate the push endpoint (clears `endpoint_stale`); also turns notifications on for a device paired without one |
| `openmosh-hook integration install\|uninstall <agent> [--settings <path>] [--json]` | Add/remove the [agent's hooks](#agents) calling `emit --source <agent>`; `--settings` overrides the default file |
| `openmosh-hook integration list [--json]` | Every supported agent: detected on this machine (binary on `PATH` or config dir present), integration installed, file, docs link |
| `openmosh-hook setup [--yes] [--no-service] [--json]` | First-time [setup](#setup-and-uninstall) |
| `openmosh-hook uninstall [--all] [--dry-run] [--yes] [--json]` | Remove everything openmosh-hook created ([details](#setup-and-uninstall)) |
| `openmosh-hook usage [--days <n>] [--json]` | Token [usage](#usage) per agent, project and model over the last `n` calendar days (1-31, default 7), from the agents' own local logs |
| `openmosh-hook doctor [--json]` | Check Tailscale, Herdr/tmux, permissions, sshd host key, sshd password logins (`sshd -G -C user=$USER,host=localhost,addr=127.0.0.1`, so `Match` blocks count; warns when password or keyboard-interactive login is on), that sshd's `AuthorizedKeysFile` includes `~/.ssh/authorized_keys`, that no untagged key was added below the first paired phone, devices, keymaps, service |
| `openmosh-hook harden-ssh [--dry-run] [--yes] [--json]` | Make sshd accept SSH keys only: pipes a fixed drop-in into `/usr/bin/sudo /usr/bin/tee /etc/ssh/sshd_config.d/10-openmosh.conf` (absolute paths, no staged file), sets it `0644`, validates it with `sshd -t` (removed again on failure) and reloads sshd. Asks before changing anything; also `s` on the TUI Status screen |
| `openmosh-hook service install\|uninstall [--dry-run] [--json]` | Register/remove the user service. The systemd unit runs with `UMask=0077`, `NoNewPrivileges`, `SystemCallFilter=@system-service`, `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`, `RestrictNamespaces`, `RestrictRealtime`, `RestrictSUIDSGID` and `LockPersonality`. Mount sandboxing (`ProtectSystem=`, `PrivateTmp=`) is not used: user services need unprivileged user namespaces for it and would not start where those are off |
| `openmosh-hook attach herdr [--session <name>]` | Run the Herdr client (`herdr` or `herdr session attach <name>`, from `[bins] herdr`) with [focus mirroring](#mirroring); exits with Herdr's status code. Session names are `[A-Za-z0-9._-]`, not starting with `-`; the mirror only uses a Herdr socket owned by the owner of `$HOME` |
| `openmosh-hook herdr snapshot [--session <name>] [--since <rev>] [--wait-ms <n>] --json` | [Navigator](#herdr-navigator) snapshot: `{"rev","unchanged":false,"workspaces":[{"id","label","focused"}],"tabs":[{"id","workspace_id","label","number","agent_status","pane_count","focused"}],"panes":[{"id","tab_id","workspace_id","agent","agent_status","focused"}]}`; with `--since` equal to the current `rev` it waits up to `--wait-ms` (max 20000) for a change and answers `{"rev","unchanged":true}` when none came |
| `openmosh-hook herdr read --pane <id> [--session <name>] [--since <rev>] [--wait-ms <n>] [--lines <n>] --json` | The pane's own text, last `--lines` lines (default 200, max 400), escape sequences and control characters removed: `{"rev","unchanged","agent","agent_status","lines":[…]}`; long-polls like `snapshot` |
| `openmosh-hook herdr action [--session <name>] --json` | Run one allowlisted [navigator action](#herdr-navigator); the request `{"op":…}` comes on stdin, the answer is `{"op","id"?}` |
| `openmosh-hook tui` | The [terminal UI](#tui) (also `openmosh-hook` alone on a terminal) |
| `openmosh-hook version [--json]` | Version and wire protocol versions (`protocol.herdr` is the navigator's) |

Ids are b64url and may start with `-`; positional id arguments accept that.

### JSON envelope

With `--json` every command prints exactly one line:

```json
{"ok":true,"data":{…}}
{"ok":false,"error":{"code":"not_found","message":"event not found"}}
```

Exit code is `0` on success and `1` on error. Messages never contain secrets. Error codes:
`invalid_input`, `invalid_request`, `not_found`, `already_resolved`, `text_too_long`, `no_pane`, `stale`, `expired`, `no_keymap`,
`runtime_unavailable`, `runtime_error`, `invalid_endpoint`, `permissions_too_open`,
`config_error`, `io_error`, `store_error`, `registry_corrupt`, `storage_error`,
`tailscale_missing`, `tailscale_not_running`, `tailscale_tagged`, `tailscale_error`, `ssh_host_key_missing`,
`listen_failed`, `pair_closed`, `pair_cancelled`, `confirmation_required`, `settings_invalid`, `service_error`, `internal`.

## Usage

`openmosh-hook usage` reads only what the agents already wrote on this machine; it never calls a
provider API and never reads credentials.

| Agent | Source | Plan limits |
|---|---|---|
| Codex | `~/.codex/sessions/**/*.jsonl`: one session per file, its usage from the last `token_count` event's `info.total_token_usage` (cached input is reported as `cache_read`) | From the newest `rate_limits` (`5h`, `weekly`) |
| Claude Code | Not read: it keeps no usage summary, and its transcripts are too large to scan | `available: false` with that `note` |
| Gemini, Cursor, Copilot, Kiro, OpenCode, Aider | — | `available: false`, `note: "usage not recorded locally"` |

The command stays cheap: it opens at most the 200 newest Codex files modified inside the period,
and from each one reads only its first and last 256 KiB (a file of up to 512 KiB is read whole),
never the middle. A session counts on the day of its last event, so a session that runs past
midnight lands entirely on the later day. Symlinks inside the directory are not followed, and
malformed or cut lines are skipped. "Today" starts at local midnight. Projects are the last folder
of the agent's working directory (top 10 by tokens), models the top 5. No message text is kept or
printed.

```json
{"ok":true,"data":{"days":7,"generated_at":1794139200,"agents":[{"agent":"codex","available":true,"note":null,
  "today":{"input":200,"output":50,"cache_read":800,"cache_write":0,"total":1050},"period":{…},
  "projects":[{"project":"App","tokens":{…}}],"models":[{"model":"gpt-5","tokens":{…}}],
  "limits":[{"name":"5h","used_percent":42.0,"window_minutes":300,"resets_at":1794150000}]}]}}
```

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
acceptable. Only then does the host ask, on its terminal,
`Pair "<name>" from <tailscale node name> (<ip>)? [y/N]`; anything but `y`/`yes`, no answer within
60 s, or Ctrl+C declines and burns the token (`pair_cancelled`). `--yes` skips the question;
without a terminal on stdin and without `--yes`, `pair` refuses with `confirmation_required`
before doing anything. The TUI runs pairing in the plain terminal, so the question works there
too. A peer whose whois did not match is not looked up again, a failing lookup is retried at most
once per second per peer, and a matching answer is remembered for the session. If the phone hangs
up before the encrypted reply can be sent (for example while the host user was deciding), the key
line and the device record are removed again. The whole request must arrive within 10 s (or the time left before expiry, if less).
Device names lose control, invisible formatting (bidi, zero-width) and line-separator characters
and are capped at 64 characters. Every failure gets the same `pair_rejected` answer; after 5
failures from a peer that passed the network checks (other addresses and other users' nodes do
not count), on timeout or on Ctrl+C the token is burned and the command exits 1. On success the phone's key is appended to `~/.ssh/authorized_keys` restricted to
the tailnet (the file is left `0600` and `~/.ssh` `0700`; a symlinked file is followed, a dangling
symlink is refused), pinned to the phone's own tailnet addresses (the connecting IPv4 plus the
node's addresses from `tailscale whois`, IPv4 and IPv6):

```text
restrict,pty,from="100.101.1.2,fd7a:115c:a1e0::1:2" ssh-ed25519 AAAA… openmosh:<device_id>
```

`restrict` turns off agent, X11 and port forwarding, `~/.ssh/rc` and any restriction a future
OpenSSH adds; `pty` keeps the interactive terminal. A phone whose Tailscale address changes (for
example after reinstalling Tailscale) has to pair again. Lines written by earlier versions keep
their whole-tailnet `from=` until the device is paired again.

**SSH (secondary).** `pair-ssh --json` registers push keys for a phone that already reaches the
host over SSH with its own key; no key is added.

The endpoint is optional: a phone without a push distributor pairs with an empty one, gets no
notifications (`devices list` shows `(no notifications)`) and adds one later with
`devices set-endpoint`. Push endpoints must be `https://`; `http://` is accepted only for a `100.64.0.0/10` address or a
`*.ts.net` name. URLs with credentials, and loopback (`127.0.0.0/8`, `::1`, `localhost`),
private (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `fc00::/7`), link-local, unspecified
(`0.0.0.0/8`, `::`), multicast or broadcast hosts are refused; Tailscale addresses
(`100.64.0.0/10`, `fd7a:115c:a1e0::/48`) are allowed. Names are resolved by the push client
itself and every forbidden address it returns is dropped, so a name cannot be re-pointed at the
LAN or the host (DNS rebinding); a name with no allowed address is not contacted. Pushes ignore
proxy environment variables, send `User-Agent: openmosh`, never follow redirects, and `404`/`410`
mark the device `endpoint_stale` (only while its endpoint is still the one that failed: one set
meanwhile with `devices set-endpoint` stays active). Push plaintexts are padded with spaces to
512, 1024 or 2048 bytes before encryption, so the ciphertext size says little about the content
(2048 is the largest bucket whose encrypted body still fits the 3 KiB limit).

`respond` claims the event atomically before sending anything, so of two concurrent answers only
one reaches the agent; the other gets `already_resolved`. Each event remembers where its pane
lives: the tmux server socket (from `$TMUX`) and the pane's `#{pane_pid}` at emit time, or the
Herdr session (`$HERDR_SESSION`, or the session in `$HERDR_SOCKET_PATH`) and, for events recorded
by `serve`, the agent's `state_change_seq`. Every `respond` call goes to that socket/session
(`tmux -S <socket>`, `herdr --session <name>`) and, right before sending, looks the pane up again:
a pane that no longer exists gets `no_pane`; a tmux pane now run by another process, or a Herdr
pane running another agent, not blocked any more (an approval) or working (a text answer), or in a
newer blocked state, gets `stale`. tmux has no agent state, so a tmux approval is accepted only
while the pane still runs the recorded process; events recorded before this check existed are
answered only when that can still be verified (Herdr: still blocked; tmux: never). An approval
older than 15 minutes gets `expired`. Refused events stay pending. Answer text loses control,
bidi, zero-width and format characters.

Approval summaries name what is being approved: the command or file path from the agent's tool
input (Claude `PermissionRequest`, and the `tool_input`/`details`/`input` objects other agents
send), as one line of at most 200 characters (`…` marks a cut) without control, bidi,
zero-width or format characters. It travels only inside the encrypted push. `integration install` run from a new
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

## Herdr navigator

The phone browses a Herdr session as structured data instead of painting the screen: `herdr
snapshot` lists workspaces, tabs and panes with each agent's status (`working`, `blocked`, `done`,
`idle`, `unknown`), `herdr read` returns one pane's text, and `herdr action` runs one action from a
fixed allowlist. Everything goes over Herdr's local socket (the one `attach herdr` uses, and only
when the owner of `$HOME` owns it) inside the phone's paired SSH session.

- **Real time without a daemon.** Each answer carries `rev`, the first 16 hex digits of the SHA-256
  of the returned fields. The phone calls again with `--since <rev> --wait-ms 20000`; the hook
  re-reads Herdr every 250 ms and answers as soon as `rev` changes, or `{"rev","unchanged":true}`
  at the deadline. Nothing runs while no phone is asking.
- **Actions** (`{"op":…}` on stdin): `workspace.create {label?}`, `workspace.rename {id,label}`,
  `workspace.close {id}`, `workspace.focus {id}`, `tab.create {workspace_id,label?}`,
  `tab.rename {id,label}`, `tab.close {id}`, `tab.focus {id}`, `pane.focus {id}`,
  `agent.start {pane_id,agent}`, `pane.keys {pane_id,keys}`, `pane.text {pane_id,text,submit}`.
  Closing happens only when asked; the phone confirms first.
- **Validation.** Unknown ops or fields are refused with `invalid_request`, as is every id that is
  malformed or not in the current snapshot, a label that is blank, over 64 characters or has
  control characters, a key outside `enter esc tab space backspace up down left right y n 1`-`9`
  (at most 8), text over 4000 characters, and an agent other than `claude`, `codex`, `gemini`,
  `cursor`, `copilot`, `kiro`, `opencode`, `amp` or `pi` (`herdr agent start` cannot start Aider).
- **Text** goes through the same path as `respond --text`: one argv entry to `herdr pane
  send-text` (with `--` when it starts with `-`), newlines turned into spaces and control characters
  removed, then Enter when `submit` is true. Keys use the socket's `pane.send_keys`, which works on
  any pane.

## Files

| Path | Mode | Content |
|---|---|---|
| `$XDG_CONFIG_HOME/openmosh/config.toml` | `0600` | `host_alias`, `retention_days` (7), `poll_interval_ms` (1000, while an agent works or waits; every 5 s when all are idle), `sources`, `ssh_port`, `ssh_host_key_pub`, `[push] server`, `[bins]` |
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
runs `install.sh --dry-run` with fake `uname`/`gh`/`minisign`/`rsign`, and real installs into a
temporary HOME against a fake `curl` that serves a fixture release (signature tag check,
checksum mismatch).

### Releases

Pushing a `v*` tag runs [`.github/workflows/release.yml`](./.github/workflows/release.yml): it
builds `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` (with `cross`),
`x86_64-apple-darwin` and `aarch64-apple-darwin`, and publishes
`openmosh-hook-<target>.tar.gz` plus one `SHA256SUMS` and its minisign signature
`SHA256SUMS.minisig` (trusted comment `openmosh-hook <tag>`) to the GitHub release. Build,
checksum, signing and publishing are separate jobs. The signing key is the repository secret
`MINISIGN_SECRET_KEY` (an unencrypted minisign secret key file), visible only to the `sign` job,
which downloads nothing but `SHA256SUMS`, runs no third-party action, installs `rsign2` from a
crate whose SHA-256 is pinned in the workflow (with its locked dependencies), writes the key to
`$RUNNER_TEMP` with umask `077`, deletes it right after signing, and checks the signature
against the committed [`minisign.pub`](./minisign.pub), so a mismatched secret fails the release.
Tags with a `-` (`v0.2.0-rc.1`) are published as GitHub pre-releases and never marked "latest",
so the installer's default skips them; see [`CONTRIBUTING.md`](./CONTRIBUTING.md#releases).
Every action in every workflow is pinned to a full commit SHA (CI fails otherwise; Dependabot
proposes updates), the release uses only `actions/*` actions plus `rustup` and `gh`, and it
caches nothing.

## License

Copyright 2026 lesquel. Licensed under the [Apache License 2.0](./LICENSE). Redistributions
must keep the [NOTICE](./NOTICE) file, which credits the author.
