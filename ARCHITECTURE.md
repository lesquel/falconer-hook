# falconer-hook — Architecture

Clean Architecture organized by feature. Global rules: dependencies point inward (`domain` imports no framework or I/O), features never import another feature's adapters, and wiring happens only in the composition root.

## Layout

```text
src/
├── main.rs                  # entry point: parse CLI, build Ctx, print the JSON envelope
├── app/                     # composition root (with main.rs): builds adapters, injects them into use cases
│   ├── mod.rs               #   Ctx (paths, env, runner), private state, runtimes, push notifier, dispatch
│   ├── commands.rs          #   request/response commands
│   ├── serve.rs             #   daemon loop
│   ├── integrations.rs      #   integration install/uninstall/list over the agent catalog
│   ├── lifecycle.rs         #   setup and uninstall
│   ├── prereqs.rs           #   what `setup` fixes on the host (tmux, SSH server), after confirmation
│   ├── harden.rs            #   `harden-ssh`: sshd drop-in for key-only logins, via absolute-path sudo
│   ├── pair.rs              #   QR and SSH pairing
│   ├── attach.rs            #   `attach herdr`: Herdr client + focus mirror
│   ├── navigator.rs         #   `herdr snapshot|read|action`: the phone's Herdr navigator
│   ├── usage.rs             #   `usage`: token usage per agent from local logs
│   ├── update.rs            #   `update`: latest release check and reinstall via the tag-pinned install.sh, after consent
│   └── tui/                 #   terminal UI: presentation only, every action is a `cli::Command` run via `run`
│       ├── state.rs         #     pure state machine (screens, selection, confirmations) over an `Exec` port
│       ├── state_tests.rs   #     unit tests of the state machine
│       ├── view.rs          #     ratatui rendering of the state
│       └── mod.rs           #     terminal setup/restore (raw mode, alternate screen, panic hook), event loop
├── cli/                     # clap definitions and the JSON envelope; no adapters
├── test_vectors.rs          # checks and regenerates tests/vectors/*.json (test builds only)
├── shared/
│   ├── domain/              # ids, clock, command-runner port, encodings, error codes, tailnet ranges,
│   │                        # runtime kind, text helpers
│   ├── config/              # XDG paths and config.toml
│   └── infrastructure/      # crypto primitives, secure fs (0700/0600, atomic writes, flock),
│                            # process runner (argv only), redacting logger, host identity, ssh fingerprint,
│                            # system clock and random ids
└── features/
    └── <feature>/
        ├── mod.rs
        ├── domain/          # entities, value objects, port traits
        ├── application/     # use cases, generic over ports
        └── infrastructure/  # adapters
tests/
├── common/                  # shared harness: temp HOME, fake herdr/tmux/tailscale/systemctl
├── cli.rs                   # the binary end to end
├── agents.rs                # agent integrations and `emit --kind`
├── lifecycle.rs             # setup and uninstall
├── herdr.rs                 # `herdr snapshot|read|action` against a fake Herdr socket
├── migration.rs             # upgrading an OpenMosh install to the Falconer names
├── prereqs.rs               # `setup` fixing tmux/SSH with fake package managers and sudo
├── usage.rs                 # `usage` against fixture agent logs
├── update.rs                # `update` with a fake curl and a logging install.sh
├── install_sh.rs            # install.sh --dry-run and fixture installs (fake uname/gh/minisign/rsign/curl)
└── vectors/                 # shared wire-format vectors (push_v1.json, pair_v1.json)
data/
└── keymaps/                 # per-agent keystrokes (versioned data, embedded at build time)
```

## Features

| Feature | Responsibility | Ports | Adapters |
|---|---|---|---|
| `agents` | Observe agent state (Herdr, tmux; Amp is observed through Herdr only), detect transitions, deliver keystrokes | `AgentRuntime` | `HerdrRuntime`, `TmuxRuntime` |
| `events` | Normalize hook payloads, dedupe, store, expire, resolve | `EventStore`, `EventNotifier` | `SqliteEventStore` |
| `push` | Encrypt per device and publish (ntfy/UnifiedPush and Web Push) | `Sealer`, `PushPublisher`, `WebPushSender` (uses `DeviceRegistry`) | `XChaChaSealer`, `HttpPublisher`, `WebPushPublisher` (`aes128gcm_sealer`, `vapid`), `PushNotifier` (implements `EventNotifier`) |
| `pairing` | QR and SSH pairing (with a host-side `y/N` confirmation), device registry, revocation that ends the phone's live SSH sessions | `DeviceRegistry`, `AuthorizedKeys`, `LiveSessions`, `Tailnet`, `PairConfirm`, `PairCrypto`, `PairSession` | `TomlDeviceRegistry`, `FileAuthorizedKeys`, `ProcSshSessions`, `TailscaleCli`, `X25519PairCrypto`, TCP listener, QR renderer |
| `responses` | Resolve an event and execute allow/deny/text | `KeymapSource` (uses `AgentRuntime`, `EventStore`, `EventNotifier`) | `EmbeddedKeymaps` |
| `service` | Render and apply the user service plan | `ServiceFiles` (uses `CommandRunner`) | `FsServiceFiles` |
| `doctor` | Diagnose dependencies, permissions and connectivity | uses other ports read-only | filesystem and service checks |
| `integration` | Agent catalog (Claude Code, Codex, Gemini, Cursor, Copilot, Kiro, OpenCode, Aider; Amp has no hook, so it is reached via Herdr only) and pure edits of each agent's hook file calling `emit` | `SettingsFile` | `JsonSettingsFile` |
| `setup` | Uninstall plan (only what falconer-hook created) and its step-by-step execution; pure decisions for the prerequisites `setup` fixes | `UninstallPorts` (implemented in `app/lifecycle.rs` over `service`, `integration`, `pairing`) | — |
| `mirror` | Run the Herdr client and mirror focus to every attached client | `FocusSocket`, `Launcher`, `MonoClock` | `HerdrSocket` (Unix socket, NDJSON), `TtyLauncher`, `SystemMonoClock` |
| `navigator` | Herdr workspaces, tabs, panes and pane text for the phone, with a fixed allowlist of actions and long-poll `rev` | `NavigatorApi` | `SocketNavigator` (Herdr socket) |
| `update` | Self-update: release tag rules, semver comparison (no prerelease offered to a stable version), install.sh arguments | uses `CommandRunner` | `Downloader` (curl/wget, HTTPS only), `PrivateTemp` (0700 dir, 0600 file), `run_installer` (`sh install.sh`) |
| `usage` | Token usage per agent, project and model from the agents' local logs (read-only) | `UsageLogs` | `FsUsageLogs` |

Cross-feature calls go through another feature's `domain` ports (for example `push` reads
`pairing::domain::DeviceRegistry`, `responses` uses `agents::domain::AgentRuntime`).

## Rules

1. `domain/` depends on `std`, `serde` and `thiserror`, plus the pure crates `base64`, `serde_json`,
   `subtle`, `url` and `zeroize` (no I/O). Crypto, SQLite, HTTP, TOML and processes live in `infrastructure/`.
2. `application/` is generic over port traits and fully unit-testable with fakes.
3. Only `main.rs` and `app/` know concrete adapter types.
4. Every subcommand used by the app prints the JSON envelope with `--json`.
5. No permanent network listener. Outbound HTTPS to push endpoints only; the pairing listener is
   one-shot and bound to the Tailscale IPv4. The mirror is a client of Herdr's local Unix socket.
6. Errors: `thiserror` in domain/application with a stable `ErrorCode`; the CLI maps them to the envelope.
7. External programs run through `CommandRunner` with argv only (no shell). The interactive
   Herdr client of `attach` is spawned by `TtyLauncher`, also argv only, with the inherited terminal.
8. The TUI never calls use cases or adapters directly: it builds the same `cli::Command` the
   CLI parses and runs it through `app::run`, so both surfaces share one code path. Text from
   commands is stripped of control characters before it is drawn.
9. No `unsafe` (`#![forbid(unsafe_code)]`); secrets are held in `zeroize` buffers.
