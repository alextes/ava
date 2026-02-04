---
schema_version: 9
id: ava-91j8
title: create telegram integration stub
priority: P2
status: done
deps:
- ava-bhwz
tags:
- telegram
owner: null
created_at: 2026-02-01T21:30:09.287641Z
started_at: 2026-02-04T20:47:19.848784Z
completed_at: 2026-02-04T20:50:26.725392Z
---

create the foundation for telegram integration:

- pull messages in from users talking to ava
- format and send messages out
- use telegram HTML parsing mode (see design issue ava-tjj4)

this is a stub — get the structure in place, actual implementation follows.