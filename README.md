# ava

<img width="1536" height="1024" alt="image" src="https://github.com/user-attachments/assets/11f4ee93-a7c4-4629-a893-4c26f21c95a5" />

hi! i'm ava — a friendly, capable ai assistant that lives on your machine and helps with whatever you need.

i can search the web, run commands, read and edit files, browse the web, remember things about you, schedule tasks, and switch between ai models mid-conversation. i talk to you through telegram (or the command line), and i keep my memory between conversations so we can build a relationship over time.

under the hood i'm a rust-based agent with a tool loop that connects to LLM providers (anthropic, deepseek, gemini, nvidia, openai, openrouter). but you don't need to worry about that — just say hi.

## getting started

clone the repo and build from source:

```bash
git clone https://github.com/alextes/ava.git
cd ava
cargo install --path .
```

## configuration

ava reads environment variables (and `.env` files via dotenvy).

### providers (at least one required)

| variable | description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | anthropic API key |
| `DEEPSEEK_API_KEY` | deepseek API key |
| `GEMINI_API_KEY` | gemini API key |
| `NVIDIA_API_KEY` | nvidia API key |
| `OPENAI_API_KEY` | openai API key |
| `OPENROUTER_API_KEY` | openrouter API key (access hundreds of models) |

any one provider key is enough to start. anthropic is the default when available. use `/switch deepseek deepseek-v4-pro`, `/switch gemini`, `/switch nvidia`, or `/switch openai` to change mid-conversation.

### optional

| variable | description |
|----------|-------------|
| `TELEGRAM_BOT_TOKEN` | telegram bot token (enables telegram channel) |
| `TELEGRAM_ALLOWED_IDS` | comma-separated user IDs allowed to DM the bot |
| `TELEGRAM_ALLOWED_CHATS` | comma-separated chat IDs for group chats |
| `TELEGRAM_BOT_NAME` | display name for mention detection in groups |
| `BRAVE_SEARCH_API_KEY` | brave search API key (enables web search) |
| `JINA_API_KEY` | jina reader API key (improves web page reading) |
| `AVA_HOME` | override `~/.ava/` home directory |
| `AVA_WORKSPACE` | override workspace root (default: cwd) |
| `AVA_BROWSER_VISIBLE` | set to `1` to show the browser window (default: headless) |

## usage

send a quick message:

```bash
ava message "what's the weather like in amsterdam?"
```

or start ava as a daemon:

```bash
ava start              # forks to background
ava start --foreground # stay in foreground (for dev)
ava stop               # stop the daemon
ava restart            # stop + start
ava logs               # tail the log file
ava logs -f            # follow mode (like tail -f)
```

other commands:

```bash
ava status     # show version, session, context usage, model
ava history    # show recent conversation history
ava skills     # list installed skills
ava rules      # manage approval rules
ava schedules  # list active scheduled tasks
ava doctor     # diagnose and repair session issues
ava upgrade    # rebuild from source and hot-swap
```

telegram turn controls:

```text
/steer <instruction>  # adjust the active turn at the next agent boundary
/stop                 # immediately stop the active turn and tool work
```

`/stop` does not stop the daemon or clear later queued messages. use `ava stop`
from the shell to stop the daemon.

## what i can do

- **exec** — run shell commands on your machine (with your approval for anything risky)
- **text_editor** — read, create, and edit files directly
- **grep / glob** — search file contents and find files by pattern
- **browser** — navigate web pages, take screenshots, click, type, read accessibility trees
- **remember / recall / forget** — store and retrieve facts, episodes, and character traits across conversations
- **skills** — load custom skills from `~/.ava/skills/` and `~/.claude/skills/` (user-invocable via `/skill-name`, model-invocable via `activate_skill`)
- **MCP tools** — connect to MCP servers configured in `~/.ava/mcp.toml`
- **cron** — schedule one-time or recurring tasks
- **tasks** — scratchpad for tracking deferred work
- **web_search** — search the web via brave search
- **web_fetch** — read web pages
- **speak** — text-to-speech via piper TTS (voice messages on telegram, local playback otherwise)
- **channel_history** — view recent messages from any monitored channel
- **manage_access** — add/remove users and chats from the whitelist
- **compact_context** — proactively compact conversation history when context is high
- **switch_model** — swap between ai providers and models mid-conversation (anthropic, deepseek, gemini, nvidia, openai, openrouter)

## how it works

- **daemon mode** — `ava start` forks to background with logs at `~/.ava/ava.log`
- **single agent loop** — all messages flow through one sequential loop, keeping conversation history clean
- **concurrent tool execution** — independent tool calls run in parallel
- **session persistence** — conversations are stored in SQLite so nothing is lost between restarts
- **context compaction** — when a conversation gets long, older messages are summarized to make room
- **workspace boundaries** — filesystem reads outside the workspace require approval
- **approval system** — shell commands require your explicit okay, with "allow always" and time-limited (15 min) patterns
- **group chat support** — mention-only mode in groups, per-chat message buffering, cross-channel context
- **initial setup** — guided first-run flow to pick a name and set character traits
- **crash recovery** — orphaned tool calls are automatically repaired on restart

## upgrading

```bash
ava upgrade
```

this rebuilds from source and hot-swaps the running process — no downtime, no lost state. ava finishes whatever it's working on, then exec's into the new binary.

> **tip:** cloning the source repo is the preferred setup if you want ava to be able to modify its own code on top of upstream releases. `ava upgrade` works with local changes too — it builds whatever's in your checkout.

## development

```bash
cargo fmt --all && cargo clippy && cargo test
```

## early days

ava is young and growing. if you've found your way here, welcome — and feel free to say hi.
