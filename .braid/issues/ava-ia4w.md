---
schema_version: 9
id: ava-ia4w
title: progress indicator for long-running telegram turns
priority: P2
status: doing
type: design
deps: []
owner: alextes
created_at: 2026-04-13T09:58:43.866124Z
started_at: 2026-04-13T10:01:12.999113Z
---

## problem

when the agent is doing a lot of work (multi-round tool loops, compaction), the user sees nothing in telegram until the final response arrives. this can take 30+ seconds and feels like the bot is broken.

## desired behavior

show a status message in telegram during long-running turns that updates in-place as state changes, then gets replaced (or deleted) when the final response arrives.

examples of what the status could show:
- "thinking..." (initial processing)
- "running tools [3/40]" (during tool loop)
- "compacting context..." (during compaction)

## existing capabilities

- `TelegramBot::edit_message_text(chat_id, message_id, text)` already exists and works (used by approval flow)
- `send_message` — need to check if it returns message_id for later editing
- no `sendChatAction` (typing indicator) support yet

## design questions to explore

1. **edit-in-place vs typing indicator vs both**: telegram's typing indicator ("typing...") is simple but limited (no custom text, auto-expires after 5s). edit-in-place gives full control but adds complexity. could combine: typing indicator immediately, then status message once tool loop starts.

2. **threading status through the agent loop**: currently `agent.process()` is a single async call that returns the final response. to emit progress, we'd need either:
   - a callback/channel pattern where the agent sends status updates as it progresses
   - a shared state object the agent writes to and the telegram layer polls
   - making the response sink available inside the agent loop

3. **cleanup**: should the status message be edited to become the final response, or deleted and replaced with a new message? editing preserves chat position but may have formatting limitations.

4. **group chats**: status messages in group chats could be noisy. consider only showing in DMs, or using a reply-to-thread approach.

## output

implementation issues covering the chosen approach.