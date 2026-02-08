# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/alextes/ava/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/alextes/ava/releases/tag/v0.1.0
