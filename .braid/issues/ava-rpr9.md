---
schema_version: 9
id: ava-rpr9
title: rich progress messages during agent processing
priority: P2
status: done
type: design
deps:
- ava-l3dy
owner: alextes
created_at: 2026-04-13T10:07:03.779113Z
done_at: 2026-04-13T10:30:00.000000Z
---

## decision: mpsc channel (approach C)

pass a `tokio::sync::mpsc::Sender<Progress>` into `agent.process()`. spawn a receiver task in the agent_loop that maps events to telegram status messages. the channel provides natural synchronization for cleanup — when the sender drops (process returns), the receiver exits, deletes the status message, and only then does the final response get sent. no race conditions.

### why C over alternatives

- **vs callback (A)**: async closures in rust are awkward. the callback would need `Box<dyn Fn(Progress) -> Pin<Box<dyn Future>>>` or write to shared state anyway.
- **vs shared state + poller (B)**: polling adds 1-2s delay and cleanup has race conditions — an in-flight edit can land after abort.
- **vs stream (D)**: biggest refactor, restructures the agent loop as an async generator. overkill for this.

### cleanup flow

```
process() returns → tx dropped → rx.recv() returns None → delete status message → await receiver → send final response
```

strict ordering, no race between status edits and final response.

### progress events

```rust
enum Progress {
    Thinking,
    ToolRound { round: u32, total: u32 },
    Compacting,
}
```

### what the user sees

1. agent starts → status message appears: "thinking..."
2. tool loop → status updates in-place: "running tools [3/40]"
3. compaction → status updates: "compacting context..."
4. agent finishes → status message deleted, final response sent

CLI and test callers pass a sender whose receiver is immediately dropped — `send()` returns `Err` which is ignored with `let _ =`. zero overhead.

## output

see implementation issues below.
