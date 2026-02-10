# ava

<img width="1536" height="1024" alt="image" src="https://github.com/user-attachments/assets/11f4ee93-a7c4-4629-a893-4c26f21c95a5" />

hi! i'm ava — a friendly, capable ai assistant that lives on your machine and helps with whatever you need.

i can search the web, run commands, read and edit files, remember things about you, schedule tasks, and switch between ai models mid-conversation. i talk to you through telegram (or the command line), and i keep my memory between conversations so we can build a relationship over time.

under the hood i'm a rust-based agent with a tool loop that connects to LLM providers (anthropic, openai). but you don't need to worry about that — just say hi.

## getting started

install from a github release:

```bash
curl -sSL https://raw.githubusercontent.com/alextes/ava/main/install.sh | bash
```

or build from source:

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
| `OPENAI_API_KEY` | openai API key (for switching models) |
| `TELEGRAM_BOT_TOKEN` | telegram bot token (enables telegram) |
| `TELEGRAM_ALLOWED_IDS` | comma-separated user IDs allowed to message the bot |
| `BRAVE_SEARCH_API_KEY` | brave search API key (enables web search) |
| `JINA_API_KEY` | jina reader API key (improves web page reading) |

## usage

send a quick message:

```bash
ava message "what's the weather like in amsterdam?"
```

or start ava as a long-running assistant:

```bash
ava start
```

this starts the agent loop and enables any configured channels (telegram, etc.). messages are processed one at a time — no crossed wires.

```bash
ava version    # show version
ava status     # show version, db path, session info
ava history    # show recent conversation history
ava schedules  # list active scheduled tasks
ava doctor     # diagnose and repair session issues
```

## what i can do

- **exec** — run shell commands on your machine (with your approval for anything risky)
- **text_editor** — read, create, and edit files directly
- **remember / recall / forget** — store and retrieve facts, episodes, and character traits across conversations
- **cron** — schedule one-time or recurring tasks
- **tasks** — scratchpad for tracking deferred work
- **web_search** — search the web via brave search
- **web_fetch** — read web pages
- **switch_model** — swap between ai providers and models mid-conversation

## how it works

- **single agent loop** — all messages flow through one sequential loop, keeping conversation history clean
- **concurrent tool execution** — independent tool calls run in parallel
- **session persistence** — conversations are stored in SQLite so nothing is lost between restarts
- **context compaction** — when a conversation gets long, older messages are summarized to make room
- **approval system** — shell commands require your explicit okay, with "allow always" patterns for commands you trust
- **crash recovery** — orphaned tool calls are automatically repaired on restart

## development

```bash
cargo fmt --all && cargo clippy && cargo test
```

## early days

ava is young and growing. if you've found your way here, welcome — and feel free to say hi.
