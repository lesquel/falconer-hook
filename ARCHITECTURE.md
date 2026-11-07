# openmosh-hook — Architecture

Clean Architecture organized by feature. Global rules: dependencies point inward (`domain` imports no framework or I/O), features never import
another feature's adapters, and wiring happens only in the composition root.

## Layout

```text
src/
├── main.rs                  # composition root: parse CLI, build adapters, inject into use cases
├── cli/                     # clap command definitions → call application use cases
├── shared/
│   ├── domain/              # ids, errors, clock port
│   └── config/              # config.toml loading (XDG paths)
└── features/
    └── <feature>/
        ├── mod.rs
        ├── domain/          # entities, value objects, port traits — no I/O crates
        ├── application/     # use cases, generic over ports
        └── infrastructure/  # adapters: rusqlite, reqwest (ntfy), herdr/tmux process calls
tests/                       # integration tests (Herdr/tmux in a container)
data/
└── keymaps/                 # per-agent approval keystrokes (versioned data, not code)
```

## Features

| Feature | Responsibility | Ports | Adapters |
|---|---|---|---|
| `agents` | Observe agent state, deliver keystrokes | `AgentRuntime` | `HerdrRuntime`, `TmuxRuntime` |
| `events` | Normalize, store and expire inbox events | `EventStore` | `SqliteEventStore` |
| `push` | Encrypt and publish payloads | `PushPublisher`, `Sealer` | `NtfyPublisher`, `XChaChaSealer` |
| `pairing` | Register/revoke devices, derive keys | `DeviceRegistry` | `TomlDeviceRegistry` |
| `responses` | Resolve an event and execute allow/deny/text | uses `AgentRuntime`, `EventStore` | — |
| `service` | Install/uninstall the user service | `ServiceManager` | `Systemd`, `Launchd` |
| `doctor` | Diagnose dependencies and connectivity | uses all ports read-only | — |

## Rules

1. `domain/` depends on `std`, `serde` and `thiserror` only.
2. `application/` is generic over port traits and fully unit-testable with fakes.
3. Only `main.rs` knows concrete adapter types.
4. Every subcommand prints JSON with `--json` so the app can call it over SSH.
5. No network listener. Outbound HTTPS to the push server only.
6. Errors: `thiserror` in domain/application, `anyhow` only in `main.rs` and `cli/`.
