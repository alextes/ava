---
schema_version: 9
id: ava-kg38
title: 'design: self-update mechanism'
priority: P3
status: open
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-09T15:17:40.70128Z
---

investigate how ava can update itself when running as a `cargo run` process from `main` on macos.

## context

scheduled tasks always produce a response, but some background work is purely internal. similarly, it would be useful for ava to be able to pull the latest code, rebuild, and restart itself without human intervention. this is especially relevant for a long-running agent that needs to stay up-to-date.

## scope

focus only on the current real use-case: updating when running as a `cargo run` process from the `main` branch on macos. other deployment scenarios (docker, systemd, remote linux) are out of scope for now.

## research summary

### approaches considered

1. **`CommandExt::exec()` (unix exec syscall)** — replaces the current process image with a new one. the PID stays the same, old code is gone. zero external dependencies. **winner.**

2. **wrapper/supervisor script** — shell script that restarts the binary in a loop. adds a moving part and friction (must always start via wrapper).

3. **launchd restart-on-exit** — system-level process management. overkill for dev setup, doesn't handle git pull + rebuild, painful to debug.

4. **fork-then-exec** — unnecessary complexity when exec alone suffices.

### recommended approach: two-step exec

```rust
pub async fn self_update() -> Result<(), Error> {
    let project_dir = env!("CARGO_MANIFEST_DIR");

    // step 1: pull and build as child process (can fail safely)
    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!("cd {project_dir} && git pull && cargo build"))
        .status()
        .await?;

    if !status.success() {
        return Err(Error::Provider("rebuild failed".into()));
    }

    // step 2: exec into the new binary (point of no return)
    // flush DB, close connections, etc. before this
    let err = std::process::Command::new("cargo")
        .args(["run", "--", "start"])
        .current_dir(project_dir)
        .exec(); // never returns on success

    Err(Error::Provider(format!("exec failed: {err}")))
}
```

key points:
- step 1 (pull + build) runs as a child process so failures are recoverable
- step 2 uses `CommandExt::exec()` to replace the process — point of no return
- `CARGO_MANIFEST_DIR` is baked in at compile time, always correct
- no wrapper scripts, no config files, no extra processes

### open questions

- should this be a tool the LLM can call, or a built-in command?
- how to handle graceful shutdown (telegram bot, open DB connections) before exec?
- should we check if there's actually a new version before pulling?
- how to handle build failures gracefully (report back to user?)
- should this only be available in certain channels (e.g. not from telegram)?