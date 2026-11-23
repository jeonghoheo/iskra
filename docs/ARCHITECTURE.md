# Iskra Agent Workbench Architecture

Status: **approved target design; not implemented as a multi-agent runtime yet**.
Date: 2026-11-23.

This document defines the architecture for turning Iskra from one persistent personal agent into a model-neutral workbench for multiple agents. It preserves the existing Rust core, local data ownership, fail-closed tools, and single-binary distribution.

The first implementation target is deliberately small: two built-in OpenAI-compatible agents, explicit `@agent` routing, independent state, per-agent FIFO execution, and bounded global concurrency. ACP, automatic model routing, agent-to-agent delegation, and hosted mode have separate gates.

## 1. Goals

- Let an owner assign work to agents with different roles, models, providers, and tool permissions.
- Make model and engine replacement possible without moving core-owned memory, policy, approvals, audit, sessions, or usage accounting into the engine.
- Complete work with measurable cost, latency, reliability, and human-intervention efficiency.
- Keep Telegram and CLI behavior working while allowing a future local web or desktop client to use the same core.
- Recover from crashes without silently repeating model calls, tools, payments, external sends, or other side effects.

## 2. Non-goals for the first slices

- A TypeScript/Electron fork or a required Node/Bun runtime.
- A database server, container, cloud account, or hosted control plane required for local use.
- Unbounded agent conversations, autonomous delegation chains, or shared write access to every workspace.
- Automatic CLI downloads, self-updating native code, or provider lock-in.
- `danger-full-access`, `approvalPolicy: never`, or an engine-specific path around core policy.
- Automatic model routing before it beats an explicit/default-agent baseline on Iskra's own evaluations.

## 3. Invariants

1. **Fail closed for authority.** Unknown senders, paths, programs, tools, engine capabilities, approvals, and state transitions are refused.
2. **Core owns authority and records.** Memory, sessions, policy, approval decisions, audit metadata, run state, and usage accounting never belong solely to an external model or CLI.
3. **No invisible replay.** A crash or restart never automatically retries interrupted work or external delivery.
4. **No third-party code in the core process.** External engines and extensions are data or separately supervised processes.
5. **One local binary for the core.** Optional external engines do not become requirements for the built-in HTTP engine.
6. **Secrets stay out of manifests and logs.** Registries contain environment-variable names or secret references, never secret values.
7. **Readable, bounded local data.** JSON, Markdown, and NDJSON use schema versions, atomic writes, private permissions, size limits, and retention limits.
8. **Routing fallback is not security fallback.** Routing may fall back to `main`; a failed permission or policy check never falls back to allow.
9. **Budgets are measured.** The existing core budgets remain: stripped binary at most 20 MB, idle core RSS at most 30 MB, and at most 15 direct dependencies. External engine resource use is reported separately rather than hidden inside or excluded silently from product measurements.

## 4. Current baseline and why a runtime is needed

At `d8b9abc`, the existing core provides one OpenAI-compatible `Brain`, one `Tools` instance, one identity, one session, and one active Telegram turn. `session/v1` persists accepted work before a model call, recovers interrupted work without replaying it, and passes real CLI process tests.

The current coupling is not multi-agent-ready:

- `ChannelState` owns one brain/tools/identity/session and one `JoinHandle`.
- `Brain::chat` obtains tool schemas from a global function.
- `Tools` advertises process capabilities through a global `OnceLock` and uses one fixed workspace.
- Memory, sticky notes, locale, and usage include global paths or process-wide state.
- A work record has no agent, run, parent task, queue, approval, artifact, or delivery identity.

Creating several `Brain` values alone would therefore mix authority and accounting. The runtime boundary comes first.

## 5. Concepts and ownership

### Agent

An Agent is a versioned configuration, not a model or a process. It contains:

- Stable lowercase `id` and display name.
- Role and instruction/persona references.
- An `engine_id` reference.
- A `workspace_id` reference.
- A tool-policy reference and memory namespace.
- Optional run/token/cost/time limits.

`main` is the reserved compatibility Agent. Agent IDs use lowercase ASCII letters, digits, and hyphens; they start with a letter and have a bounded length. The core rejects duplicates, unknown references, reserved-path collisions, and invalid IDs before accepting work.

### Engine profile

An Engine profile describes how to execute a run:

- `openai_compatible`: base URL, model, and a secret reference such as `api_key_env`.
- `acp_process` (later): an owner-approved executable, fixed argument policy, environment allowlist, and capability settings.

Agents refer to engines by ID. Changing a model or provider changes an Engine profile or the Agent's reference, not core state ownership. Missing credentials and unavailable capabilities produce an explicit unavailable state, never a silent fallback to another paid provider.

### Task and Run

- A **Task** is the owner-visible requested outcome.
- A **Run** is one Agent's execution attempt for a Task.
- A retry is a new Run linked to the same Task; it is never disguised as the same attempt.
- A delegated Run has a parent Task/Run and a separately bounded budget.
- Every Task has exactly one accountable primary Agent, even when later delegation is enabled.

### Workspace

A Workspace is an owner-approved file root identified by ID. Models never choose arbitrary host paths.

- `main.workspace` retains the existing `workspace/` root.
- Additional agents receive independent sibling roots under `agent-workspaces/<agent-id>/`.
- One Agent's root must not contain another Agent's root.
- A future owner-approved external project root requires an explicit registry entry and canonical-path validation.
- Cross-agent file exchange goes through the core Artifact Broker, not through a shared writable directory by default.

## 6. Component boundaries

```text
Telegram / CLI / future local web or desktop
                  |
             Channel Adapter
       commands in / RuntimeEvents out
                  |
              Core API
                  |
        Agent + Engine Registries
                  |
      Explicit -> Rules -> Optional Model Router
                  |
       Run Supervisor + Durable Queues
       admission / budgets / cancellation
                  |
        +---------+----------+
        |                    |
 OpenAI-compatible       ACP Process Adapter
 built-in Engine         (optional, later)
        |                    |
        +---------+----------+
                  |
           Canonical RuntimeEvent
                  |
 Memory / Session / Policy / Approval / Usage / Audit / Artifacts
                  |
               Core-owned
```

### Core API

The Core API is the only entry point for accepting work, changing run state, granting approval, recording usage, and publishing artifacts. Channels normalize input and render output; they do not decide permissions or independently persist execution state.

### Agent and Engine Registries

Registries validate configuration and construct supervised runtime instances. Unknown or disabled engines remain unavailable snapshots; they do not crash the fleet. Runtime construction must not mutate credentials or install software.

### Run Supervisor

The Supervisor owns active tasks/process handles, per-agent queues, global admission, cancellation, restart recovery, and run events. It replaces channel-owned `JoinHandle` management.

Cancellation order:

1. Persist the requested cancellation state before acknowledging it when the state store is writable.
2. Signal the engine and terminate its supervised process group where applicable.
3. Fold late events without reopening or completing a cancelled Run.
4. If state persistence fails, fail closed and do not start replacement work.

### Engine Adapter

The Engine Adapter boundary provides, at minimum:

- Static and discovered capabilities.
- Start a Run from a core-approved `RunContext`.
- Emit normalized events with monotonic per-run sequence numbers.
- Interrupt a Run.
- Respond to an engine permission/elicitation request when supported.
- Report native resume/session references and usage when actually available.

`RunContext` carries the filtered tool schemas and matching executor, Agent and Run IDs, canonical workspace, memory/session handles, model selection, approval mode, and budget. An engine must not call the global tool schema function directly.

An unsupported operation returns `unsupported` or `unavailable`; it must not be emulated by weakening policy.

### Channel Adapter

Channels translate owner input into commands and RuntimeEvents into messages/cards. They do not own model configuration, long-term memory, tool policy, or run completion. Telegram typing indicators and future streaming displays are projections of run activity, not proof of completion.

### Policy and Approval Broker

The broker evaluates every core-mediated action using the Agent's current policy. Approval requests identify the Agent, Run, action type, bounded human-readable summary, and expiration. The default for timeout, missing answerer, malformed request, or unavailable broker is denial.

`allow_once` never becomes a persistent grant. Any broader grant requires an explicit, separately specified owner action. External engine permission requests pass through the same broker when ACP support exists.

### Artifact Broker

Artifacts are explicit Run outputs with an ID, origin Run, bounded size, and metadata. Sharing an artifact requires an owner-authorized Task relationship or an explicit owner action. The broker validates paths and never exposes arbitrary host files or secret stores.

## 7. Run state machine

Primary states:

```text
queued -> preparing -> running <-> waiting_approval
                           |
                           +-> completed
                           +-> failed
                           +-> cancelled
                           +-> interrupted

queued -> cancelled
queued_paused -> queued       (explicit owner resume only)
queued -> queued_paused       (restart recovery)
preparing/running/waiting_approval -> interrupted (restart recovery)
```

Rules:

- Persist `queued` before acknowledging accepted work.
- Persist `preparing` before spawning an external engine and `running` before the first model or tool action.
- A queue write failure means no model call and no tool execution.
- A terminal-state write failure poisons the relevant runtime store against further work until it is writable and valid again.
- Only the expected current Run ID and event sequence may advance state; late duplicate events are harmless observations.
- `completed` means the core received a valid final result. It does not guarantee Telegram or network delivery.
- No state transition automatically authorizes a retry, payment, external send, or new Agent.

Delivery is a separate projection:

```text
pending -> sent
pending -> failed
pending -> unknown
```

`unknown` is not retried blindly. A future channel-specific reconciliation may resolve it without rerunning the Task.

## 8. Queue, concurrency, and routing

### Queue semantics

- Each Agent has one durable FIFO queue by default.
- A Run is claimed atomically so a restart or a second dispatcher cannot dispatch the same queue item twice.
- Queue settlement is separate from dispatch. A failed or interrupted dispatch leaves an explicit state rather than losing the item.
- The first multi-agent slice does not coalesce messages. Later coalescing requires identical owner, route, mode, and reply context, plus tests proving no words are hidden from the executing Agent.
- Owner-interactive work has priority over future scheduled/routine work.

### Admission

- Initial global concurrent Run limit: **2**, configurable after measurement.
- Per-agent default: **1** active Run.
- Future per-provider limits and rate limits are additional constraints, never reasons to exceed the global limit.
- Queue length, output size, execution time, and per-Task delegation have hard bounds.
- Limits report backpressure to the owner; they do not silently drop work.

### Routing order

1. A valid explicit `@agent` or API `agent_id`.
2. A deterministic owner-configured rule.
3. An optional evaluated decision model.
4. `main` as the non-security fallback.

A router chooses an Agent; it never executes the Task itself. The selected Agent uses its own Engine, context, and budget. Invalid router output, timeout, missing provider, or low confidence returns to the configured default. Router prompts and responses receive the same privacy and logging limits as other model traffic.

## 9. Local data and migration

The target local layout is:

```text
iskra.json                         # local versioned Agent/Engine registry; no secrets
iskra.example.json                 # public, credential-free template (when implemented)
SOUL.md / USER.md                  # existing main persona and shared owner context
workspace/                         # existing main workspace only
agent-workspaces/<agent-id>/       # additional independent roots
memory/
  MEMORY.md                        # existing main long-term memory
  session.json                     # existing main session/v1
  runs.json                        # versioned Run/queue state (when implemented)
  agents/<agent-id>/
    MEMORY.md
    SOUL.md
    session.json
logs/
  agent.log                        # existing operational metadata
  runs/<run-id>.ndjson             # bounded event metadata, when implemented
```

All user-specific registry, workspace, memory, run, and log paths remain ignored by Git. Registry and Run stores use private atomic writes. A missing registry means the compatibility `main` Agent synthesized from the existing `LLM_*` configuration; no file is created merely to answer a read-only status request.

Session migration rules:

- Existing `session/v1` data is read as `main` history.
- A future schema migration preserves an atomic backup and is validated before the new store becomes active.
- Unknown versions, invalid UTF-8/JSON, unknown fields under a strict schema, impossible transitions, and invalid ID references fail closed.
- No migration silently merges two Agents' histories or promotes session text into long-term memory.
- Long-term Markdown memory remains distinct from session replay and Run event logs.

RuntimeEvents may carry transient response text in memory for a streaming channel. Persisted NDJSON contains metadata by default: event ID/sequence, timestamp, Run/Agent IDs, event kind, structured outcome, tool name, and usage. It does not duplicate prompts, secrets, full tool output, or private responses into another log.

## 10. External ACP engines (separate gate)

ACP provides a standard JSON-RPC-over-stdio boundary for local agent subprocesses. Iskra should prefer one well-tested ACP adapter over a growing set of provider-specific process protocols, while retaining the built-in HTTP engine.

Before implementation:

- Select one real CLI and document its ACP version, capabilities, permission requests, cancellation, resume behavior, and usage reporting.
- Evaluate the official Rust SDK's maintenance and license and measure dependency, binary, and RSS effects. Adding it requires separate approval; the initial contract work adds no dependency.
- Pin an explicit owner-installed executable. Do not automatically download or update provider code.
- Use a dedicated workspace, cleared/allowlisted environment, OS sandbox where required, output limits, timeouts, and process-group termination.
- Map engine permission requests to the Approval Broker. If tools cannot be observed and constrained, use chat-only mode or refuse the engine configuration.
- Report subscription-based token/cost information as unknown unless the engine actually supplies it.

A transport or protocol error after a model/tool action is an indeterminate side-effect boundary. It must not trigger an automatic fresh Run.

## 11. Efficiency and evaluation

Efficiency is a measured product property, not a model-name claim. Record by Task/Run:

- Verified Task success.
- Wall time and p50/p95 latency.
- Input/output/cached tokens when available.
- Actual or clearly labelled estimated cost; unknown for unavailable subscription data.
- Tool errors, retries, delegation count, queue wait, and cancellation.
- Human approvals, corrections, and manual repair required.

Start with deterministic routing and fixed model profiles. An automatic router is enabled by default only if an Iskra-labelled evaluation set shows that it improves success/cost/latency over the explicit/default baseline without unacceptable privacy loss. Do not import another project's probability threshold without recalibration.

## 12. Implementation stages

### Stage 0 — design and current gate

- Publish this architecture and the companion business strategy.
- Re-run the existing tests and resource measurements.
- Complete owner-run Telegram and real-provider checks for `session/v1` before writing multi-agent execution code.

### Stage 1 — compatibility-preserving runtime

- Add Agent, Engine, Run, RuntimeEvent, and Supervisor boundaries.
- Wrap the existing `Brain` as the first built-in Engine.
- Inject tool schemas/executors and instance-owned memory/workspace/session context.
- Move Telegram `JoinHandle` ownership into the Supervisor.
- Preserve all current user-visible behavior and add no dependency or parallelism.

### Stage 2 — two-Agent vertical slice

- Add the local registry, independent Agent state, `/agents`, explicit `@agent`, per-agent FIFO, and bounded global admission.
- Keep plain messages on `main`; no decision-model charge.
- Add localized `en/ko/ru` run/queue/status/cancellation text.
- Do not enable free-form agent-to-agent messaging, automatic delegation, or automatic retries.

### Stage 3 — ACP and bounded delegation

- Implement one validated ACP path behind core approval and sandbox boundaries.
- If delegation is later approved, use depth 1, bounded child count/time/tokens, and one accountable primary Agent.

### Stage 4 — optimization and product surfaces

- Evaluate rules and decision-model routing.
- Add reusable procedures/skills and credential-free portable packages.
- Consider local web/Tauri and hosted mode only after local reliability and retention are demonstrated.

## 13. Verification requirements

Every code stage runs the full existing test suite, real CLI process tests, changed-file rustfmt, clippy, release build, and resource measurements. No completion claim relies only on compilation or an import.

Stage 1 must prove unchanged current behavior, including replay, reasoning echo, stop recovery, tool sandboxing, and usage recording.

Stage 2 must additionally prove:

- Every allowed and forbidden Run transition.
- FIFO order, atomic claim/settle, concurrency limits, and cancellation races.
- No model/tool request after restart until the owner explicitly resumes.
- Different Agents use their configured model and isolated memory/session/workspace.
- One Agent cannot read, write, advertise tools for, or charge usage to another Agent.
- Corrupt registry/state fails closed without resetting data.
- Logs and status distinguish Agent/Run/Engine/model and do not contain secrets or prompt contents.
- Real owner-run Telegram and configured-provider checks pass before the feature is presented as live-verified.

The ACP gate must prove approval allow/deny, unsupported-capability refusal, process-group cleanup, external file/network restrictions, and no automatic side-effect replay with both a mock ACP process and one real installed CLI.

## 14. Reuse and license boundary

OpenMausBot's non-`enterprise/` code is Apache 2.0 and may be selectively ported with its license, copyright, NOTICE, and modification obligations. Any actual port records the source repository, exact commit, source files, destination files, and modifications. The `enterprise/` tree, trademarks, and mascot are excluded.

OpenBot is PolyForm Noncommercial 1.0.0. Iskra independently implements general ideas such as independent workspaces and durable queues; it does not copy OpenBot code, text, tests, or assets into the commercial product.

References:

- https://github.com/milind-soni/OpenMausBot
- https://github.com/milind-soni/OpenMausBot/blob/main/LICENSING.md
- https://github.com/milind-soni/OpenMausBot/blob/main/server/contracts.ts
- https://github.com/milind-soni/OpenMausBot/blob/main/server/channel-queue.ts
- https://github.com/nightly-labs/openbot
- https://github.com/nightly-labs/openbot/blob/main/LICENSE
- https://agentclientprotocol.com/get-started/introduction
