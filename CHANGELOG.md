# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/alextes/ava/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/alextes/ava/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/alextes/ava/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/alextes/ava/releases/tag/v0.1.0
