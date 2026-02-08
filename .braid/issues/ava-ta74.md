---
schema_version: 9
id: ava-ta74
title: research exec tool prior art across agent harnesses
priority: P2
status: done
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-08T19:51:29.948941Z
started_at: 2026-02-08T19:54:03.885692Z
completed_at: 2026-02-08T21:45:37.363943Z
---

research how other agent harnesses handle exec/shell tools, with focus on dangerous command blocking and working directory handling.

## ava's current exec tool

- single `exec` tool: `sh -c <command>` with optional `timeout_secs`
- basic blocklist: `rm -rf /`, `rm -rf /*`, `mkfs`, `dd if=`, `> /dev/sd`, fork bombs
- all exec calls require approval (telegram: inline keyboard; CLI: auto-approve)
- no `cwd` parameter — always runs in process working directory
- no persistent shell — each command spawns a fresh `sh -c` process
- output truncated at 4000 chars, timeout default 30s / max 300s
- sensitive env detection warns about `ANTHROPIC_API_KEY`, `TELOXIDE_TOKEN`

## per-harness findings

### claude code

- **execution**: persistent bash session (long-lived `/bin/bash` process with stdin/stdout pipes). state (env vars, cwd) persists across commands.
- **tool params**: `command`, `timeout` (default 120s, max 600s), `description`, `run_in_background`, `dangerouslyDisableSandbox`
- **no cwd param**: working directory tracked via temp file mechanism — every command silently appends `pwd -P >> /tmp/claude-*-cwd`. `cd` persists across calls. `CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR=1` resets cwd after each command.
- **no hardcoded blocklist**: relies on configurable permission rules (deny/ask/allow arrays in `settings.json`) and PreToolUse hooks. the system prompt instructs the model to avoid dangerous patterns, but enforcement is user-configured.
- **approval**: 5 modes — `default` (prompt on first use), `acceptEdits` (auto-approve edits), `plan` (read-only), `dontAsk` (deny unless pre-approved), `bypassPermissions` (skip all prompts). rule syntax: `Bash(npm run *)` with glob matching. shell-operator-aware (detects `&&` bypass attempts).
- **sandboxing**: OS-level via seatbelt (macOS) and bubblewrap (linux). write access restricted to cwd by default. network routed through proxy with domain allowlists. reduced permission prompts by 84%. open-sourced as `@anthropic-ai/sandbox-runtime`.
- **output**: 30,000 char truncation (middle-cut, keeps beginning + end). background tasks via `run_in_background` + `TaskOutput` polling.

### codex (openai)

- **execution**: two modes — fresh processes (default `shell` tool) or persistent PTY sessions (`exec_command` behind `UnifiedExec` feature flag). max 64 concurrent processes in unified mode.
- **tool params**: `command`, `workdir` (optional cwd), `timeout_ms`. unified exec adds `shell`, `tty`, `max_output_tokens`.
- **has cwd param**: `workdir` on the tool + `--cd` CLI flag + `--add-dir` for multi-project.
- **explicit safe/dangerous lists**: safe commands auto-approved (`cat`, `ls`, `grep`, `pwd`, `wc`, etc.). dangerous commands always flagged (`git reset`, `git push --force`, `rm -f`, `sudo`). custom `.rules` files for user-defined patterns with prefix matching.
- **approval**: two-axis system — sandbox mode (what's technically possible) x approval policy (when to ask). policies: `on-request`, `untrusted`, `on-failure`, `never`. `--full-auto` = `on-request` + `workspace-write` sandbox.
- **sandboxing**: OS-level — seatbelt (macOS), bubblewrap+seccomp (linux), restricted tokens (windows). `.git` and `.codex` dirs carved out as read-only. network disabled by default.
- **env filtering**: strips vars matching `*KEY*`, `*SECRET*`, `*TOKEN*` by default.
- **output**: 1 MiB per stream. head+tail buffer (50/50 split, middle dropped). token-based truncation (~4 bytes/token).
- **timeout**: 10s default yield, 30s max yield, 2s IO drain timeout.

### goose (block)

- **execution**: built-in MCP server (`developer` extension). `tokio::process::Command` per call, no state persistence. sets `GIT_TERMINAL_PROMPT=0`, `GIT_PAGER=cat`, overrides `EDITOR` to prevent interactive prompts.
- **tool params**: `command` only. no cwd param — working directory passed via MCP metadata header `agent-working-dir` (set per-session by agent host).
- **no blocklist**: uses `.gooseignore` (gitignore-style patterns) to block access to sensitive file paths (`.env`, `secrets.*`, SSH keys). no static command deny list.
- **approval**: 4 modes — `auto` (default, no confirmation), `smart_approve` (read-only auto, write ops like `rm`/`cp`/`mv` require confirmation), `approve` (all require confirmation), `chat` (no tool use).
- **no sandboxing**: open feature request (#5943). commands run with full user privileges.
- **output**: 400,000 char hard limit. >100 lines → temp file. streamed via MCP logging notifications.

### cline

- **execution**: VS Code integrated terminal (shell integration API) or background child processes. terminal sessions persist cwd.
- **tool params**: `command`, `requires_approval` (boolean, set by the model per command based on system prompt instructions).
- **no cwd param**: cwd is workspace root. terminal reuse logic: same dir + shell + not busy → reuse; different dir → `cd` then reuse; else → new terminal.
- **no static blocklist**: the LLM self-assesses risk and sets `requires_approval`. the system prompt instructs: `true` for installs, deletes, system config, network ops; `false` for reads, dev servers, builds. `CommandPermissionController` supports env-var-configured allow/deny glob patterns and detects dangerous shell constructs (backticks, line separators, redirects).
- **approval**: per-action-type toggles in UI — auto-approve safe commands, auto-approve all commands, or manual for everything. 30s notification timeout for long-running auto-approved commands.
- **no sandboxing**.
- **output**: multi-tier — 500 lines to model (2000 for subagent), 1000 lines/512KB → file logging, 1MB memory cap.

### aider

- **execution**: no tool/function call — model suggests commands in ` ```bash ``` ` blocks, parsed from markdown. pexpect (unix) or subprocess (windows). each command spawns fresh shell with `cwd=project_root`.
- **no cwd param**: always project root.
- **no blocklist**: no dangerous command detection at all. purely user-confirmation-based.
- **approval**: user must explicitly confirm each AI-suggested command (`confirm_ask("Run shell command?", explicit_yes_required=True)`). user-initiated `/run` commands execute immediately.
- **no sandboxing**.
- **output**: no truncation — full output captured. user asked whether to add output to chat context (shows token count).
- **no timeout**.

### openhands

- **execution**: tool call (`execute_bash`) → `CmdRunAction` → executed in Docker container via REST API. persistent tmux session inside container — env vars, virtualenvs, cwd all persist across commands.
- **tool params**: `command`, `is_input` (for stdin to running process), `timeout`, `security_risk` (LLM self-assesses: LOW/MEDIUM/HIGH/UNKNOWN).
- **has cwd param**: but only for `is_static=True` (one-off isolated commands). normal commands use stateful cwd tracked via PS1 metadata.
- **multi-statement rejection**: `bashlex` parses commands and rejects multi-statement input — must chain with `&&`/`;`.
- **multi-layer security**: (1) LLM self-assessed risk on each call, (2) optional separate LLM security analyzer, (3) pattern-based custom analyzers (e.g. flag `rm -rf`, `sudo`, `chmod 777`), (4) ConfirmRisky policy blocks actions above threshold.
- **sandboxing**: Docker container with volume mounts for project files. resource limits via `RUNTIME_MAX_MEMORY_GB`.
- **output**: 30,000 char truncation (middle-cut). tmux history limit 10,000 lines.
- **timeout**: 30s soft timeout (no new output), optional hard timeout per command.

### cursor

- **execution**: `run_terminal_cmd` tool in VS Code integrated terminal. persistent shell session — cwd persists across calls.
- **tool params**: `command`, `is_background`, `require_user_approval` (model sets this), `explanation`.
- **no cwd param**: relies on persistent shell session.
- **user-configurable denylist**: not a built-in static list. security researchers proved denylists are mathematically broken — infinite bypass variants via base64, subshells, script wrapping, quote escaping. CVE-2026-22708 demonstrated env var poisoning via trusted builtins.
- **approval**: 3 modes — sandbox (auto-execute inside sandbox), ask every time, run everything. model sets `require_user_approval` per command.
- **sandboxing**: seatbelt (macOS), landlock+seccomp (linux). write restricted to workspace. acknowledged as "best-effort, not a security boundary."
- **output**: truncated (~85KB dropped from beginning). stderr capture unreliable. pager commands require `| cat`.

## comparative table

| harness | cwd param | persistent shell | blocklist | sandboxing | approval model | output limit | timeout |
|---------|-----------|-----------------|-----------|------------|---------------|-------------|---------|
| **claude code** | no | yes | no (configurable rules) | seatbelt/bubblewrap | 5-mode tiered | 30K chars | 120s default, 600s max |
| **codex** | yes (`workdir`) | optional (feature flag) | yes (safe + dangerous lists) | seatbelt/bubblewrap/seccomp | 2-axis (sandbox x policy) | 1 MiB | 10-30s |
| **goose** | via MCP header | no | no (`.gooseignore` paths only) | none | 4-mode | 400K chars | none |
| **cline** | no | yes (terminal) | no (model self-assesses) | none | per-action toggles | 500 lines to model | none |
| **aider** | no | no | none | none | user confirm each | none | none |
| **openhands** | yes (static only) | yes (tmux) | custom patterns | docker | LLM risk + custom analyzers | 30K chars | 30s soft |
| **cursor** | no | yes | configurable denylist | seatbelt/landlock | 3-mode | ~85KB | none |

## design question 1: dangerous command blocking

### industry patterns

four approaches, roughly in order of sophistication:

1. **no blocking** (aider, goose) — rely purely on user approval for every command. simplest but high friction.
2. **static blocklist** (ava current, codex safe/dangerous lists) — block known-dangerous patterns, auto-approve known-safe ones. codex has the most mature implementation with separate safe and dangerous lists plus recursive script analysis.
3. **model-assessed risk** (cline, cursor, openhands) — the LLM itself sets a risk flag per command. flexible but gameable via prompt injection.
4. **sandbox-first** (claude code, codex, cursor) — OS-level isolation makes blocking less important. let commands run but restrict filesystem/network access. industry moving this direction.

### key insight

security researchers proved static denylists are **mathematically broken** — for every blocked command, infinite bypass variants exist (base64 encoding, subshells, script wrapping). claude code moved from blocklist to allowlist after security audits. sandbox-first is the emerging consensus.

### recommendation for ava

**keep the existing blocklist but don't invest heavily in expanding it.** ava's blocklist catches the most obvious catastrophic commands (`rm -rf /`, `mkfs`, fork bombs) which is fine as a basic safety net. the real protection comes from the approval system — every exec call already requires approval.

expanding the blocklist into codex-style safe/dangerous classification would be useful if ava ever adds auto-approval for safe commands (e.g. auto-approve `ls`, `cat`, `cargo test` without prompting). this pairs naturally with the existing auto-approval rules system.

sandboxing (seatbelt/bubblewrap) would be the highest-impact safety improvement but is a large effort — defer unless ava moves toward autonomous execution modes.

## design question 2: working directory argument

### industry patterns

three approaches:

1. **no cwd param, persistent shell** (claude code, cursor, cline) — `cd` persists across calls. the model manages cwd by running `cd` commands. most common approach.
2. **explicit cwd param** (codex `workdir`, openhands `cwd` for static commands) — tool accepts a directory argument. cleaner for one-off commands in different directories.
3. **fixed project root** (aider, goose) — always runs in project root. model uses `cd dir && command` for other directories.

### recommendation for ava

**add a `cwd` parameter to the exec tool.** ava spawns a fresh `sh -c` process per command (no persistent shell), so `cd` doesn't persist between calls. without a cwd param, the model has to do `cd /some/dir && actual-command` every time, which is verbose and error-prone. a `cwd` parameter is simple to implement (pass to `tokio::process::Command::current_dir()`) and matches codex's approach.

```json
{
  "type": "object",
  "properties": {
    "command": { "type": "string" },
    "timeout_secs": { "type": "integer" },
    "cwd": { "type": "string", "description": "working directory for the command (default: process cwd)" }
  },
  "required": ["command"]
}
```

implementation: add `cwd: Option<String>` to `ExecInput`, pass to `Command::new("sh").current_dir(cwd)` in `execute_command()`.

## summary

ava's exec tool is in a reasonable spot. two concrete improvements:

1. **add `cwd` parameter** — simple, high value, matches codex pattern
2. **keep blocklist as-is** — expand to safe/dangerous classification only when adding auto-approval for safe commands