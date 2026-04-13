---
schema_version: 9
id: ava-ia4w
title: progress indicator for long-running telegram turns
priority: P2
status: done
type: design
deps: []
owner: alextes
created_at: 2026-04-13T09:58:43.866124Z
started_at: 2026-04-13T10:01:12.999113Z
done_at: 2026-04-13T10:15:00.000000Z
---

## problem

when the agent is doing a lot of work (multi-round tool loops, compaction), the user sees nothing in telegram until the final response arrives. this can take 30+ seconds and feels like the bot is broken.

## outcome

split into two issues:
- **ava-l3dy**: typing indicator during agent processing (quick win, implement now)
- **ava-rpr9**: rich progress messages design (design issue, compare callback vs other patterns)
