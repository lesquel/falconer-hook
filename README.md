# openmosh-hook

Host-side daemon and CLI for [OpenMosh](https://github.com/lesquel/OpenMosh). It watches your AI
agents (through Herdr or tmux), keeps an inbox of events, sends **end-to-end encrypted** push
notifications through ntfy, and executes approvals sent from your phone over SSH.

It opens **no network port**.

> Status: v0.1 in development.

## Install

See [`INSTALL.md`](https://github.com/lesquel/OpenMosh/blob/main/docs/INSTALL.md). Planned one-liner:

```sh
curl -fsSL https://raw.githubusercontent.com/lesquel/openmosh-hook/main/install.sh | sh
```

## Commands (planned)

| Command | Purpose |
|---|---|
| `openmosh-hook serve` | Run the daemon |
| `openmosh-hook emit --source <id> --event <json>` | Inject an event from an agent hook |
| `openmosh-hook pair --device-key <b64>` | Pair a phone (called by the app over SSH) |
| `openmosh-hook inbox list --json` | List events |
| `openmosh-hook respond <event-id> allow\|deny\|text <msg>` | Answer an agent |
| `openmosh-hook devices list\|revoke <id>` | Manage paired phones |
| `openmosh-hook service install\|uninstall` | Manage the user service |
| `openmosh-hook doctor` | Diagnose the setup |

## Architecture

Clean Architecture by feature. See [`ARCHITECTURE.md`](./ARCHITECTURE.md).

## Development (once scaffolded)

```sh
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## License

Proposed Apache-2.0 (pending confirmation).
