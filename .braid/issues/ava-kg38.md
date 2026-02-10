---
schema_version: 9
id: ava-kg38
title: 'design: self-update mechanism'
priority: P3
status: done
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-09T15:17:40.70128Z
completed_at: 2026-02-09T17:47:33.438536Z
---

investigate how ava can update itself. the broader direction is to run ava as a background daemon, which changes how start, stop, update, and logging work.

## context

ava runs as a long-lived process. when new code is pushed to `main`, it has no way to update itself — requires manual intervention. adding a self-update mechanism lets the agent (or user) trigger an update that pulls latest code, rebuilds, and replaces the running process.

## scope

focus on the current real use-case: macos, running from the `main` branch. other deployment scenarios (docker, systemd, remote linux) are out of scope for now.

## resolved decisions

- **daemon model**: `ava start` forks to background (traditional unix daemon), writes PID file, returns control to shell. `ava stop` sends SIGTERM.
- **update flow**: build-then-hot-swap — `ava update` pulls code, builds as child process, then signals the running daemon to `exec()` into the new binary. minimal downtime.
- **core mechanism**: `CommandExt::exec()` (unix exec syscall) replaces the current process image. PID stays the same, old code is gone. zero external dependencies.
- **approval**: no approval needed for self-update tool
- **DB safety**: trust SQLite WAL — crash-safe, no explicit cleanup before exec()
- **version check**: always pull, no pre-check needed

## research summary

### approaches considered for restart

1. **`CommandExt::exec()`** — replaces process image atomically. zero deps. **winner.**
2. **wrapper/supervisor script** — adds friction (must always start via wrapper).
3. **launchd restart-on-exit** — overkill, doesn't handle git pull + rebuild.
4. **fork-then-exec** — unnecessary complexity when exec alone suffices.

### two-step exec pattern

```rust
// step 1: pull and build as child process (recoverable on failure)
let status = Command::new("sh")
    .arg("-c")
    .arg(format!("cd {project_dir} && git pull && cargo build"))
    .output();

// step 2: exec into new binary (point of no return)
let err = Command::new(&binary_path)
    .arg("start")
    .exec(); // never returns on success
```

- step 1 runs as child process — failures are recoverable
- step 2 uses exec() — process is atomically replaced
- `CARGO_MANIFEST_DIR` baked in at compile time for project directory

## new subcommands to design

- `ava start` — fork to background, write PID file, log to file
- `ava stop` — read PID file, send SIGTERM
- `ava logs` — tail the log file
- `ava update` — pull, build, signal daemon to exec() into new binary

## resolved: open questions

### PID file location → `~/.ava/ava.pid`

ava already uses a home-relative pattern conceptually. `~/.ava/` is a natural home for runtime state. XDG runtime dir is linux-specific and we're scoped to macos. `~/.ava/` is simple and conventional for macos CLI tools.

### log file location → `~/.ava/ava.log`, no rotation initially

keep it simple. a single log file alongside the PID file. rotation can be added later if the file gets large. `ava logs` just tails this file. `tracing_appender` supports writing to a file.

### signal for exec into new binary → SIGUSR1

SIGUSR1 is the simplest option. flow:
1. `ava update` pulls code, builds as child process
2. on success, `ava update` sends SIGUSR1 to the running daemon (reads PID from file)
3. daemon's SIGUSR1 handler sets an atomic flag
4. after current message completes, daemon calls `exec()` into the new binary with `start` arg

avoids control socket complexity and is more explicit than overloading SIGTERM.

### graceful shutdown before exec → yes, drain current message

the agent loop processes one message at a time. on SIGUSR1, set an atomic flag. after the current `agent.process()` completes, check the flag and exec() instead of pulling the next message. clean boundary — no partial responses. waiting messages are fine to lose since telegram re-delivers them (same offset).

### `ava start` duplicate check → yes

check PID file existence + process liveness (via `kill(pid, 0)`). if already running, print a message and exit. prevents accidental double-starts which would cause conflicts.

### self-update as LLM tool → yes

the resolved decisions say "no approval needed for self-update tool". exposing it lets the agent update itself when asked. the tool triggers the same flow as the CLI command — pull, build, SIGUSR1.