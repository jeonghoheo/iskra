# Iskra (Искра)

**English** | [Русский](README.ru.md) | [한국어](README.ko.md)

![Iskra mascot](assets/iskra-mascot.gif)

**A lightweight personal AI agent, built from scratch in Rust.**
*Iskra* (Искра) is Russian for "spark" — the small flame everything starts from.

![License: FSL-1.1-ALv2](https://img.shields.io/badge/License-FSL--1.1--ALv2-blue.svg)
![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)
![Status](https://img.shields.io/badge/Status-v1%20in%20development-yellow.svg)

Iskra is a personal agent that lives in a messenger, remembers across restarts,
uses tools under strict permission boundaries, and is designed to be taken apart
and reassembled — swap the model, the channel, or the tools without touching the core.

> **Status:** v1 is under active development. The terminal-based core loop,
> file tools and memory work today; the Telegram channel is next.
> This repository contains code only — personal memory, data and secrets never live here.

## Design principles

1. **Light by budget, not by hope** — a single binary with hard limits on size,
   idle memory and dependency count, measured at every release.
2. **Modular by contract** — brain, channel, tools and memory connect through
   versioned interface specs (`brain/v1`, `channel/v1`, `tool/v1`, `memory/v1`).
   Extension means adding parts, never editing the core.
3. **Fail-closed safety** — tools run inside a sandbox folder, shell access is
   allowlist-only, secrets are structurally kept out of logs and memory, and
   irreversible actions require human approval.
4. **Cost discipline** — repeated work gets crystallized into procedures that
   run without a model call; expensive senses (screen) are the last resort,
   structured channels (API · DOM) come first.
5. **Memory you can read** — long-term memory is plain Markdown with a load
   budget, credential redaction on save, and infection defenses: content from
   outside never enters core memory without the owner's review.

## Current state (v1)

- [x] Rust skeleton + DeepSeek brain (direct HTTP, no SDK)
- [x] Conversation loop with stop conditions (max tool round-trips, repeated-error halt)
- [x] File tools (`read_file`, `write_file`) sandboxed to `workspace/`
- [x] Markdown memory with budgeted load + redacted save, identity files (SOUL.md / USER.md)
- [ ] Telegram channel with sender allowlist
- [ ] Allowlisted shell tool + a verified coding task

## Roadmap

| Version | Direction |
|---|---|
| v1 | A personal agent for one user (Telegram, one brain, four tools, Markdown memory) |
| v2 | Swappable parts — contract specs formalized, multiple brains, decision-model routing |
| v3 | Delegated work — triggers, bot packages, identity spec (`identity/v1`) |
| v4 | Desktop app (Tauri) — multi-bot, computer use behind approval gates |
| v5 | Product — hosted mode, per-user memory + adapter serving, signed releases |

## Quick start

```bash
git clone https://github.com/jeonghoheo/iskra.git
cd iskra
cp .env.example .env   # add your DEEPSEEK_API_KEY
cargo run
```

## License

Source-available under the **Functional Source License, Version 1.1
(FSL-1.1-ALv2)** — see [LICENSE](LICENSE). You may use, study, modify and
self-host Iskra freely; offering it to others as a competing commercial
product or service is not permitted. Each release converts to Apache 2.0
two years after its publication.
