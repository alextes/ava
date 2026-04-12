---
schema_version: 9
id: ava-yafn
title: design local voice I/O channel (mic + headphones as input/output)
priority: P2
status: open
type: design
deps:
- ava-tk07
- ava-7zcb
- ava-9f2a
tags:
- voice
- design
owner: null
created_at: 2026-03-24T16:24:27.965915Z
---

## goal

design a mode where ava uses the host machine's mic and speakers/headphones as a bidirectional voice channel. user speaks into mic (STT via parakeet), ava responds through speakers/headphones (TTS via piper). this is separate from the telegram voice message flow.

## context

ava already has (or will have) the building blocks:
- STT: `transcribe` tool using parakeet-mlx (ava-tk07)
- TTS: `speak` tool using piper (ava-g0iw)

this issue is about connecting them as a continuous local I/O channel — think "hey ava" conversational mode where the user is at their machine with headphones and a mic.

## key design questions

- how to capture audio from the system mic? (portaudio? cpal? shell out to a recorder?)
- voice activity detection — when does the user start/stop talking?
- wake word or push-to-talk or always-listening?
- how does this interact with the existing telegram channel? (parallel? exclusive?)
- latency budget — STT + LLM + TTS end-to-end target?
- how to start/stop voice mode? (`ava voice` CLI command?)
- interruption handling — can the user interrupt ava while it's speaking?

## not in scope for this design

- telegram voice messages (handled by ava-g0iw / speak tool)
- the speak and transcribe tools themselves (separate issues)
