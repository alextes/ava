---
schema_version: 9
id: ava-21is
title: add secret declarations to skill frontmatter
priority: P2
status: done
deps:
- ava-9smj
tags:
- skill
- secret
owner: null
created_at: 2026-03-19T08:42:18.441479Z
started_at: 2026-03-20T09:20:22.864006Z
completed_at: 2026-03-20T09:22:59.445212Z
---

extend the Skill struct and SKILL.md parsing to support secret declarations in frontmatter.

## format

```yaml
secrets:
  - name: PROD_DB_URL
    source: vault://prod-db-url
    sensitivity: medium
  - name: DEPLOY_KEY
    source: op://Private/deploy-key
    sensitivity: high
```

## source types

- vault:// — reads from ~/.ava/vault/<name>
- op:// — reads from 1Password CLI (high sensitivity, separate issue)

## sensitivity levels

- medium — telegram approval button unlocks
- high — requires biometric auth (separate design issue)

## implementation

- add secrets field to Skill struct (Vec<SkillSecret>)
- parse from YAML frontmatter
- SkillSecret { name, source, sensitivity }
- sensitivity defaults to medium if omitted