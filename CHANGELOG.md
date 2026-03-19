# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0]

### Added
- **skill system**: load skills from `~/.ava/skills/*/SKILL.md` with YAML frontmatter. user invocation via `/skill-name`, model invocation via `activate_skill` tool, skill descriptions in system prompt. `ava skills` CLI to list installed skills
- **MCP integration**: connect to MCP servers over stdio, namespaced tool discovery, config via `~/.ava/mcp.toml`, automatic restart-on-failure
- **browser tool**: chromiumoxide-based browser with navigate, screenshot, click, type, and accessibility tree snapshot actions
- **daemon mode**: `ava start` forks to background by default. plain-text log file at `~/.ava/ava.log` (no ANSI codes). `ava stop` sends SIGTERM and waits. `ava logs` tails the log file with `-f` follow mode. `ava restart` for stop+start
- **context usage tracking**: `ContextUsage` struct with model-aware window sizes. injected into tool results at key thresholds (60% once, 80% every round). auto-compaction raised to 90%. persisted to DB (migration v9). shown in `ava status`
- **workspace boundaries**: filesystem tools (text_editor, grep, glob) restricted to workspace directory. reads outside workspace require approval
- **grep and glob tools**: ripgrep-based codebase search and glob file matching
- **image support**: base64 image content blocks in messages and tool results
- **self-upgrade tool**: `ava upgrade` rebuilds from source with SIGUSR1 hot-swap
- **approval improvements**: per-segment pattern matching for piped/chained commands, path-based edit rules, `/rules` slash command, `ava rules` CLI subcommand, grep/glob patterns in approval prompts
- **OpenAI Responses API**: migrated from Chat Completions, with reasoning token logging
- **provider fallback**: automatic provider switch on budget exhaustion, `/switch` slash command for manual switching
- **`~/.ava/` home directory**: PID file, log files, skills, and MCP config all under `~/.ava/`. `AVA_HOME` env var to override

### Changed
- system prompt trimmed to identity only, context usage moved to message injection for prompt cache efficiency
- default models upgraded to claude-sonnet-4-6 and gpt-5.4
- compaction threshold raised from 80% to 90%, context injection thresholds tuned (60% heads-up, 80% warning)
- WARN log threshold raised to 80% context usage
- main.rs, agent/mod.rs, and tool/mod.rs split into smaller modules

### Fixed
- include cache_read_tokens in context usage percentage calculation
- model-aware context window sizes (opus/sonnet 1M, haiku 200k, gpt-5.4 1.05M, gpt-5-mini 400k)
- telegram approval UX for expired and resolved messages
- strip env var prefixes in pattern generation and matching
- clear invalid persisted model instead of silently falling back
- rate-limit vs budget exhaustion error distinction

## [0.3.1]

### Changed
- rename `TELOXIDE_TOKEN` env var to `TELEGRAM_BOT_TOKEN`
- update README to reflect all new capabilities added in 0.3.0

## [0.3.0]

### Added
- **cron tool**: schedule one-time and recurring tasks with natural language
- **tasks tool**: scratchpad for deferred work items
- **complete tool**: silent task completion signaling
- **text_editor tool**: filesystem handler for `text_editor_20250728`
- **cwd parameter**: exec tool now accepts a working directory
- **concurrent tool calls**: tool calls execute in parallel with `futures::join_all`
- **`ava history` command**: view conversation history with `--full` flag and color output
- **`ava schedules` command**: list active schedules
- **`ava doctor` command**: diagnose and repair session issues
- **scheduler task board check**: built-in periodic task review
- **graceful tool loop limit**: final LLM summary turn instead of hard cutoff
- **date/time in system prompt**: agent knows the current date and time
- **telegram HTML formatting**: convert markdown to telegram HTML before sending

### Changed
- share a single `reqwest::Client` across providers and tools
- rename `doctor fix` to `doctor repair-orphans`
- simplify auto-repair flow

### Fixed
- orphaned tool call recovery messaging reworded to neutral framing
- scan full history for orphaned `tool_use` blocks, not just the tail
- repair orphaned `tool_use` blocks automatically on message load
- handle approval timeout gracefully instead of crashing
- resolve clippy warnings on rust 1.93

## [0.2.0]

### Added
- **manage_rules tool**: list and delete approval rules, plus agent-proposed `action=add` for suggesting new rules
- **auto-approval from stored rules**: telegram approver checks saved rules before prompting the user
- **built-in tool types**: provider serialization supports anthropic built-in tools (e.g. text_editor)

### Changed
- tool loop limit increased from 5 to 20
- split large `tool/mod.rs` and `db/mod.rs` into focused submodules
- README rewritten in ava's voice

## [0.1.0]

### Added
- **unified agent loop**: single sequential message queue (`ava start`) replacing per-message task spawning, preventing interleaved conversation history
- **multi-provider support**: anthropic (default) and openai providers with `switch_model` tool for mid-conversation switching
- **session persistence**: SQLite-backed conversation history with growing window for prompt cache efficiency
- **context compaction**: automatic summarization when approaching model context limits
- **unified memory system**: `remember_fact`, `forget`, and `recall` tools for persistent knowledge across sessions, with character traits, facts, and episodic memory
- **model persistence**: selected model persists across restarts within a session
- **tool system**: exec (shell commands with approval flow), web_search (brave), web_fetch (jina reader), remember_fact, switch_model
- **telegram channel**: bot integration with user ID whitelist, inline keyboard approval for dangerous commands, HTML formatting with plain text fallback
- **prompt caching**: cache breakpoints in anthropic API calls for efficiency
- **context usage observability**: token usage and cache hit/miss logging
- **approval system**: auto-approve for CLI, interactive approve/deny/allow-always for telegram with pattern-based rules
- **structured logging**: tracing with configurable log levels
- **install script**: curl-pipe-bash installer for github releases
- CLI commands: `message`, `start`, `version`, `status`

### Fixed
- openai provider uses `max_completion_tokens` instead of deprecated `max_tokens`

[Unreleased]: https://github.com/alextes/ava/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/alextes/ava/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/alextes/ava/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/alextes/ava/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/alextes/ava/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/alextes/ava/releases/tag/v0.1.0
