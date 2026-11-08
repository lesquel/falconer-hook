# openmosh-hook — Architecture

Clean Architecture organized by feature. Global rules: dependencies point inward (`domain` imports no framework or I/O), features never import another feature's adapters, and wiring happens only in the composition root.

## Layout

```text
src/
├── main.rs                  # entry point: parse CLI, build Ctx, print the JSON envelope
├── app/                     # composition root (with main.rs): builds adapters, injects them into use cases
│   ├── mod.rs               #   Ctx (paths, env, runner), private state, runtimes, push notifier, dispatch
│   ├── commands.rs          #   request/response commands
│   ├── serve.rs             #   daemon loop
│   └── pair.rs              #   QR and SSH pairing
├── cli/                     # clap definitions and the JSON envelope; no adapters
├── shared/
│   ├── domain/              # ids, clock, command-runner port, encodings, tailnet ranges, runtime kind
│   ├── config/              # XDG paths and config.toml
│   └── infrastructure/      # crypto primitives, secure fs (0700/0600, atomic writes, flock),
│                            # process runner (argv only), redacting logger, host identity, ssh fingerprint
└── features/
    └── <feature>/
        ├── mod.rs
        ├── domain/          # entities, value objects, port traits
        ├── application/     # use cases, generic over ports
        └── infrastructure/  # adapters
tests/
├── cli.rs                   # the binary against a temp HOME and fake herdr/tmux/tailscale/systemctl
├── install_sh.rs            # install.sh --dry-run
└── vectors/                 # shared wire-format vectors (push_v1.json, pair_v1.json)
data/
└── keymaps/                 # per-agent keystrokes (versioned data, embedded at build time)
```

## Features

| Feature | Responsibility | Ports | Adapters |
|---|---|---|---|
| `agents` | Observe agent state, detect transitions, deliver keystrokes | `AgentRuntime` | `HerdrRuntime`, `TmuxRuntime` |
| `events` | Normalize hook payloads, dedupe, store, expire, resolve | `EventStore`, `EventNotifier` | `SqliteEventStore` |
| `push` | Encrypt per device and publish | `Sealer`, `PushPublisher` (uses `DeviceRegistry`) | `XChaChaSealer`, `HttpPublisher`, `PushNotifier` (implements `EventNotifier`) |
| `pairing` | QR and SSH pairing, device registry, revocation | `DeviceRegistry`, `AuthorizedKeys`, `Tailnet`, `PairCrypto`, `PairSession` | `TomlDeviceRegistry`, `FileAuthorizedKeys`, `TailscaleCli`, `X25519PairCrypto`, TCP listener, QR renderer |
| `responses` | Resolve an event and execute allow/deny/text | `KeymapSource` (uses `AgentRuntime`, `EventStore`, `EventNotifier`) | `EmbeddedKeymaps` |
| `service` | Render and apply the user service plan | `ServiceFiles` (uses `CommandRunner`) | `FsServiceFiles` |
| `doctor` | Diagnose dependencies, permissions and connectivity | uses other ports read-only | filesystem and service checks |
| `integration` | Add/remove Claude Code hooks calling `emit` | `SettingsFile` | `JsonSettingsFile` |

Cross-feature calls go through another feature's `domain` ports (for example `push` reads
`pairing::domain::DeviceRegistry`, `responses` uses `agents::domain::AgentRuntime`).

## Rules

1. `domain/` depends on `std`, `serde` and `thiserror`, plus the pure crates `base64`, `serde_json` and
   `subtle` (no I/O). Crypto, SQLite, HTTP, TOML and processes live in `infrastructure/`.
2. `application/` is generic over port traits and fully unit-testable with fakes.
3. Only `main.rs` and `app/` know concrete adapter types.
4. Every subcommand used by the app prints the JSON envelope with `--json`.
5. No permanent network listener. Outbound HTTPS to push endpoints only; the pairing listener is
   one-shot and bound to the Tailscale IPv4.
6. Errors: `thiserror` in domain/application with a stable `ErrorCode`; the CLI maps them to the envelope.
7. External programs run through `CommandRunner` with argv only (no shell).
8. No `unsafe` (`#![forbid(unsafe_code)]`); secrets are held in `zeroize` buffers.
