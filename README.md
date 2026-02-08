# ava

a capable agent harness for personal assistance. ava connects to LLM providers (anthropic, openai) and processes messages through a single sequential agent loop — no interleaved conversations, no race conditions.

## install

from a github release:

```bash
curl -sSL https://raw.githubusercontent.com/alextes/ava/main/install.sh | bash
```

or from source:

```bash
cargo install --git https://github.com/alextes/ava.git
```

## configuration

ava reads environment variables (and `.env` files via dotenvy).

### required

| variable | description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | anthropic API key (default provider) |

### optional

| variable | description |
|----------|-------------|
| `OPENAI_API_KEY` | openai API key (for `switch_model` tool) |
| `TELOXIDE_TOKEN` | telegram bot token (enables telegram channel) |
| `TELEGRAM_ALLOWED_IDS` | comma-separated user IDs allowed to message the bot |
| `BRAVE_SEARCH_API_KEY` | brave search API key (enables `web_search` tool) |
| `JINA_API_KEY` | jina reader API key (improves `web_fetch` results) |

## usage

### one-shot message

```bash
ava message "what's the weather like in amsterdam?"
```

### start all channels

```bash
ava start
```

starts a long-running process with a single agent loop. channels (telegram, etc.) push messages into a shared queue; the agent processes them sequentially. channels are enabled based on which env vars are set.

### other commands

```bash
ava version   # show version
ava status    # show version, db path, session info
```

## tools

ava has built-in tools the LLM can use:

- **exec** — run shell commands (requires approval via CLI auto-approve or telegram buttons)
- **remember_fact** — store facts for future conversations (e.g. user preferences)
- **web_search** — search the web via brave search
- **web_fetch** — fetch and read web pages via jina reader
- **switch_model** — switch LLM provider/model mid-conversation

## architecture

- **single agent loop** — all inbound messages flow through one sequential processing loop, preventing interleaved conversation history
- **message queue** — channels push to a shared `tokio::mpsc` queue; the agent loop is the sole consumer
- **session persistence** — conversation history is stored in SQLite with a growing window for prompt cache efficiency
- **context compaction** — when approaching the model's context limit, older messages are summarized to free space
- **approval system** — dangerous tool calls (shell commands) require explicit approval, with "allow always" patterns for trusted commands

## development

```bash
cargo fmt --all && cargo clippy && cargo test
```
