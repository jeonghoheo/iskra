# Iskra (Искра)

**English** | [Русский](README.ru.md) | [한국어](README.ko.md)

![Iskra mascot](assets/iskra-mascot.gif)

**A lightweight personal AI agent, built from scratch in Rust.**
*Iskra* (Искра) is Russian for "spark" — the small flame everything starts from.

![License: FSL-1.1-ALv2](https://img.shields.io/badge/License-FSL--1.1--ALv2-blue.svg)
![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)
![Status](https://img.shields.io/badge/Status-v2%20in%20progress%20%C2%B7%20two--agent%20runtime-blue.svg)

Iskra is a personal agent that lives in a messenger, remembers what you tell it — immediately and across restarts,
uses tools under strict permission boundaries, and is designed to be taken apart
and reassembled — swap the model, the channel, or the tools without touching the core.

> **Status:** v1 is complete and in real-use testing. The first v2 slice — a two-agent
> workbench with durable runs and explicit routing — is implemented and live-verified.
> ACP engines, bounded delegation, and decision-model routing remain later stages.
> This repository contains code only — personal memory, data and secrets never live here.

## Design principles

1. **Light by budget, not by hope** — a single binary with hard limits on size,
   idle memory and dependency count, measured at every release.
2. **Modular by contract** — brain, channel, tools and memory are designed to
   connect through versioned interface specs (`brain/v1`, `channel/v1`, `tool/v1`,
   `memory/v1`); the specs are formalized as code in v2. Extension means adding parts, never editing the core.
3. **Fail-closed safety** — tools run inside a sandbox folder; the shell tool
   allows only listed programs, subcommands and arguments, runs with a cleared
   environment, and on macOS runs inside an OS sandbox with no network access;
   credentials are redacted before anything is saved to memory; human approval
   for irreversible actions is planned for v2.
4. **Cost discipline** — (planned) repeated work gets crystallized into procedures that
   run without a model call; expensive senses (screen) are the last resort,
   structured channels (API · DOM) come first.
5. **Memory you can read** — long-term memory is plain Markdown with a load
   budget, credential redaction on save, and infection defenses — today as rules
   the model is given, with a structural quarantine for outside content planned for v2.

## Current state (v1 complete · v2 in progress)

- [x] Rust skeleton + brain (direct HTTP, no SDK) — the model is a swappable part; v1 ships with DeepSeek as the default
- [x] Conversation loop with stop conditions (max tool round-trips, repeated-error halt)
- [x] File tools (`read_file`, `write_file`) sandboxed to `workspace/`
- [x] Markdown memory with budgeted load + redacted save, re-read on every message (no restart needed), identity files (SOUL.md / USER.md)
- [x] Telegram channel with sender allowlist
- [x] Shell tool (`ls`, `cat`, `git`, `cargo`) with subcommand and argument policy, cleared environment and macOS sandbox (cargo is disabled where no OS sandbox exists) + a verified coding task
- [x] Shell hardening (v1.1): 21 bypass paths closed and covered by tests
- [x] Two-agent runtime (Stage 2): a local `iskra.json` registry (at most two agents), an isolated model, tools, memory, Session, workspace and usage record per agent, a durable run queue in `memory/runs.json`, explicit `@agent` routing, `/agents`, `/status [@agent]`, `/stop [@agent]`, `/resume [@agent]`, and `@name` attribution on replies from another agent
- [ ] Next up: Stage 2.5 delivery reliability (`delivery/v1`) — approved, not implemented — then one validated ACP engine path and bounded delegation (Stage 3); decision-model routing and product surfaces remain later stages

## Known limitations

- The Telegram channel reaches the Bot API directly or through an HTTP proxy configured
  with `HTTPS_PROXY` / `ALL_PROXY` (and `NO_PROXY` for exclusions). SOCKS5 is supported;
  prefer `socks5h://` so the proxy resolves DNS. `TELOXIDE_PROXY` is not used, and proxy
  settings from macOS System Settings are not read — export the environment variables
  before starting Iskra.
- If the Bot API is unreachable at startup, Iskra retries for up to about 20 seconds,
  writes `telegram_fatal` to `logs/agent.log`, and exits with code 2. If connectivity
  drops while running, it keeps retrying with exponential backoff (1 s to 64 s), stays
  alive, and records the state in `memory/telegram_health.json`; it does not push a
  notification. Check the health file and the process state.
- Proxy operation on a real Russian network is not covered by automated tests; verify it
  once with your own proxy before relying on it.
- The two-agent flow is covered by automated tests (a mock Bot API plus two local
  OpenAI-compatible servers). An owner-run Stage 2 checklist passed on 2026-11-24
  (routing and isolation, global concurrency 2 and per-agent FIFO, targeted
  status/stop/resume, restart recovery without replay). The post-remediation live
  checklist passed on 2026-11-24 as well (board replies, restart `/resume` delivery,
  targeted `/stop`, second-dispatcher refusal, Korean UI, and reply attribution;
  en/ru rendering remains test-covered).
  A plain message always goes to `main`; only a leading `@name` selects another agent.
  Forwarded text, stickers and sticky-board replies stay on `main`. Replies from
  another agent start with `@name`; `main` replies have no label.
- One Iskra process may own a data directory at a time. A second process started in
  the same working directory refuses to start before any model call or store write;
  stop the first instance instead of running two.
- Additional-agent roots are derived from the agent ID (`agent-workspaces/<id>/`,
  `memory/agents/<id>/`) and must be real directories. A symlinked root, or one that
  aliases `main`, another agent, or an outside path, is refused at startup; the
  registry cannot point at arbitrary paths.
- With a second agent, configure `iskra.json` locally in the working directory where
  Iskra runs (it is Git-ignored). The exact registry shape is in
  `docs/ARCHITECTURE.md` §12; a missing registry keeps the existing single-`main` setup.

## Roadmap

| Version | Direction |
|---|---|
| v1 | A personal agent for one user (Telegram, one brain, five tools, Markdown memory) |
| v2 (in progress) | Swappable parts — two-agent runtime, durable runs and explicit routing are done; delivery reliability (`delivery/v1`) is next, then ACP engines, bounded delegation and decision-model routing |
| v3 | Delegated work — triggers, bot packages, identity spec (`identity/v1`) |
| v4 | Desktop app (Tauri) — multi-bot, computer use behind approval gates |
| v5 | Product — hosted mode, per-user memory + adapter serving, signed releases |

## Quick start

```bash
git clone https://github.com/jeonghoheo/iskra.git
cd iskra
cp .env.example .env   # add your model API key (LLM_API_KEY — any provider; DeepSeek is the default)
cargo run
```

To add a second agent, create a local `iskra.json` in the working directory where
Iskra runs (see `docs/ARCHITECTURE.md` §12 for the shape) and put that engine's key in
the environment variable named by `api_key_env`. Without `iskra.json`, Iskra runs the
single `main` agent as before.

## License

Source-available under the **Functional Source License, Version 1.1
(FSL-1.1-ALv2)** — see [LICENSE](LICENSE). You may use, study, modify and
self-host Iskra freely; offering it to others as a competing commercial
product or service is not permitted. Each release converts to Apache 2.0
two years after its publication.
