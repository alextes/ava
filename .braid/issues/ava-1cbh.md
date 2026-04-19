---
schema_version: 9
id: ava-1cbh
title: fix flaky AVA_HOME env-var race in vault/secrets tests
priority: P2
status: doing
deps: []
tags:
- test
- bug
owner: alextes
created_at: 2026-04-19T06:14:40.237024Z
started_at: 2026-04-19T06:14:43.954278Z
---

cargo test occasionally fails `tool::exec::tests::test_load_vault_secrets_no_dir` with `assertion failed: secrets.is_empty()`. root cause: three tests mutate process-global AVA_HOME concurrently:

- src/tool/exec.rs:307 test_load_vault_secrets_from_dir
- src/tool/exec.rs:325 test_load_vault_secrets_no_dir
- src/secrets.rs:180 test_find_env_op_files

under cargo's default parallel runner, one test's set_var can overwrite another's before it reads back via ava_home_dir() (src/config.rs:27).

fix: introduce a shared `static ENV_LOCK: Mutex<()>` and have each env-touching test acquire the guard for the duration of the set/read/unset block. keeps scope minimal; no new deps.