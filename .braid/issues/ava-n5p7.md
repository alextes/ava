---
schema_version: 9
id: ava-n5p7
title: design skills system
priority: P2
status: done
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-04T21:37:09.324262Z
started_at: 2026-02-10T13:27:03.200516Z
completed_at: 2026-02-10T13:36:22.142326Z
---

markdown files that inject context-specific instructions based on triggers. follow the agent skills open standard ([agentskills.io](https://agentskills.io/specification)) adopted by claude code, codex, copilot, cursor, windsurf, and others.

## prior art

an open standard has emerged for agent skills. anthropic published the agent skills spec in december 2025, now adopted across the industry. the core idea: a skill is a directory containing a `SKILL.md` file with YAML frontmatter + markdown instructions.

key convergence points across all tools:
- **format**: markdown with YAML frontmatter
- **progressive disclosure**: metadata (name + description) loaded at startup for all skills (~100 tokens each), full body loaded only on activation
- **dual invocation**: user-explicit (`/name`) and model-implicit (description-based)
- **scoping**: personal (`~/`) and project (`.`) directories
- **arguments**: `$ARGUMENTS` substitution in body

### claude code specifics

claude code has the most mature implementation:
- skills in `.claude/skills/<name>/SKILL.md` (directory-based, can include scripts/references/assets)
- legacy `.claude/commands/<name>.md` still works (flat files)
- frontmatter: `name`, `description`, `allowed-tools`, `disable-model-invocation`, `user-invocable`, `context: fork`, `agent`
- dynamic context via `` !`command` `` shell preprocessing
- context budget: 2% of context window for all skill descriptions

### codex (openai)

- same SKILL.md format in `.agents/skills/`
- explicit and implicit activation modes

### cursor

- uses `.mdc` files in `.cursor/rules/` (flat, not directory-based)
- unique `globs` field for file-pattern activation
- `alwaysApply` boolean

## design for ava

### what ava needs vs what IDE tools need

ava is a personal assistant, not an IDE. the differences:
- no file-editing context (no globs/applyTo)
- no subagent forking (single agent loop)
- simpler scope — just personal skills (`~/.ava/skills/`)
- telegram is the primary interface, not a terminal

what we keep: the core format, progressive disclosure, dual invocation, argument substitution.

### file format

follow the agent skills standard. a skill is a directory with a `SKILL.md`:

```
~/.ava/skills/
├── morning-briefing/
│   └── SKILL.md
├── code-review/
│   └── SKILL.md
└── research/
    └── SKILL.md
```

SKILL.md format:

```yaml
---
name: morning-briefing
description: give a morning briefing with weather, calendar, and news headlines. use when the user says good morning or asks for a briefing.
---

check the weather for amsterdam, review today's calendar, and summarize top news.
present everything in a concise morning briefing format.
```

### frontmatter fields

| field | required | description |
|:------|:---------|:------------|
| `name` | no | display name. defaults to directory name. |
| `description` | yes | what it does + when to use it. the LLM reads this to decide relevance. max 1024 chars. |
| `user-invocable` | no | if `false`, hidden from user-facing skill list, only ava can invoke. default `true`. |
| `disable-model-invocation` | no | if `true`, only the user can trigger (explicit only). default `false`. |

we omit fields that don't apply to ava: `allowed-tools`, `context`, `agent`, `globs`, `model`.

### discovery and loading

on startup, scan `~/.ava/skills/*/SKILL.md`. parse frontmatter from each. store in memory as a `Vec<Skill>` with:

```rust
struct Skill {
    name: String,
    description: String,
    user_invocable: bool,
    disable_model_invocation: bool,
    body: String,  // full markdown body (loaded lazily or eagerly — files are small)
}
```

no hot-reloading needed initially. restart to pick up new skills.

### activation

**two modes, matching the standard:**

1. **user-explicit**: user sends `/morning-briefing` (or via telegram command). the skill body is prepended to the user's message as context.

2. **model-implicit**: all non-`disable-model-invocation` skill descriptions are injected into the system prompt in a `## available skills` section. the LLM sees what's available and can invoke a skill via a new `activate_skill` tool that returns the skill body.

### prompt injection

**for model-invocable skills:**

add a section to the system prompt:

```
## available skills

you have access to the following skills. use the activate_skill tool when a skill is relevant to the user's request.

- morning-briefing: give a morning briefing with weather, calendar, and news headlines. use when the user says good morning or asks for a briefing.
- research: deep research on a topic with web search and synthesis. use when the user asks you to research something thoroughly.
```

**context budget**: cap the descriptions section at 2000 chars (ava has shorter system prompts than IDE tools). if skills exceed the budget, truncate descriptions.

**for user-invoked skills:**

when the user sends `/skill-name [args]`, prepend the skill body to their message:

```
[skill: morning-briefing]
<skill body here>
[/skill]

<user's additional message or args>
```

### argument substitution

support `$ARGUMENTS` in skill body, replaced with everything after the skill name:

```
/research quantum computing
```

in the skill body: `$ARGUMENTS` → `quantum computing`

### the activate_skill tool

a new tool the LLM can call:

```json
{
  "name": "activate_skill",
  "description": "load a skill's full instructions. call this when a skill is relevant to the current request.",
  "input_schema": {
    "type": "object",
    "properties": {
      "name": {
        "type": "string",
        "description": "skill name to activate"
      }
    },
    "required": ["name"]
  }
}
```

returns the skill body as the tool result. the LLM then follows those instructions to handle the user's request.

## resolved questions

### is LLM confirmation worth the latency/cost?

**no.** the original aidaemon approach proposed pattern-match then LLM confirmation. the industry has moved past this — just put descriptions in the system prompt and let the LLM decide via a tool call. one round-trip is cheaper and more reliable than a separate confirmation step.

### could use simpler regex or exact match triggers?

**not needed.** the description-based approach is more flexible. the LLM understands intent, not just keywords. for explicit invocation, exact name match (`/skill-name`) is sufficient.

### skill composition — can skills reference other skills?

**no, not initially.** keep it simple. a skill is a self-contained instruction set. if composition is needed later, the LLM can invoke multiple skills in sequence via the tool.

### skill parameters — can triggers pass data to skill body?

**yes, via `$ARGUMENTS` substitution.** matches the standard.

### user-defined vs built-in skills?

**user-defined only for now.** built-in behavior lives in the system prompt and tools. skills are for user customization. we could ship example skills in a separate repo later.

### skill priority/ordering when multiple match?

**not an issue.** the LLM decides which skill to activate. if the user invokes explicitly, it's unambiguous. no priority system needed.

## implementation issues to create

1. **add skill loading from `~/.ava/skills/`** — parse SKILL.md files, frontmatter extraction, `Skill` struct
2. **inject skill descriptions into system prompt** — add `## available skills` section, context budget
3. **add `activate_skill` tool** — tool definition + handler that returns skill body
4. **add user-explicit skill invocation** — detect `/skill-name` prefix in messages, prepend skill body, `$ARGUMENTS` substitution
5. **add `ava skills` CLI subcommand** — list installed skills (name + description)