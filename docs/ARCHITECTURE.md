# Iskra Agent Workbench Architecture

Status: **approved target design including the future Outcome-first Headless Control Plane; the Stage 2 two-Agent slice is committed as `3d3c98f`, and Stage 2.5-A `delivery/v1` is complete with automated and owner-run verification. Outcome/Headless/BYOUI/Lease, Stage 3 and later are not implemented**.
Date: 2026-11-25.

This document defines the architecture for turning Iskra from one persistent personal agent into a model-neutral workbench and, behind later gates, an Outcome-first Headless Control Plane. It preserves the existing Rust core, local data ownership, fail-closed tools, and single-binary distribution.

The compatibility runtime, first two-Agent slice, and Stage 2.5-A delivery reliability are implemented. A completed Run does not imply that its answer reached Telegram. No later code Stage starts automatically: the next code card follows transaction and repeated-paid-demand gates. ACP, Outcome/Headless APIs, BYOUI projections, automatic model routing, agent-to-agent delegation, leases and hosted mode remain behind separate gates.

## 1. Goals

- Let an owner define a measurable **Outcome** with acceptance and evidence, then decompose it into Tasks and Runs with different roles, models, providers and tool permissions.
- Make model and engine replacement possible without moving core-owned memory, policy, approvals, audit, sessions, or usage accounting into the engine.
- Measure **Verified Outcome Rate**, total cost per Verified Outcome, time to evidence, human intervention, repeat achievement and delivery assurance—not merely Task/token counts.
- Keep Telegram and CLI behavior working while allowing SaaS backends, other agents, a thin web console and customer-generated UIs to use the same scoped Core projection. Headless means any UI, not no UI.
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
10. **Delivery is not execution success.** A completed Run means the core received a valid final result. Whether that result reached a channel is a separate Core-owned delivery record, and an ambiguous external send is never blindly retried.
11. **Outcome achievement is not Task completion.** A future Outcome becomes `achieved` only after its declared acceptance checks and required evidence pass. Task/Run completion, generated text and a client receipt are each insufficient alone.
12. **Projections are authorized views, not new authorities.** Every UI receives a scoped, versioned projection of canonical state. Generated or external UIs never write stores directly; commands and approvals pass through Core policy. Sync exports use the minimum necessary fields and explicit platform-scope warnings.

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

### Outcome (target contract, not implemented)

An Outcome is the customer-visible desired result and the top-level contract for future Headless and BYOUI clients. It contains an owner, desired result, baseline/target metric and direction, deadline, budget, allowed Agents/tools/workspaces, approval policy, data scopes, client idempotency key, result-delivery policy, and acceptance checks with required evidence/evaluator.

The target hierarchy is:

```text
Outcome
  └── Task
        └── Run
              └── Event / Artifact / Evidence / Delivery
```

A vague objective without a metric or acceptance is not promoted into an Outcome; Intake asks a bounded question or keeps it as a Task. Target states are `draft → proposed → active → evaluating → achieved`, with `blocked`, `failed` and `cancelled`. Task/Run completion never automatically achieves an Outcome. Approval records authorize privileged effects but are not acceptance evidence; achievement requires the declared non-LLM acceptance checks and required evidence. External ambiguity remains `unknown`, and reopening requires explicit evidence/event history rather than a silent reset.

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

- A **Task** is an actionable unit of work linked to an Outcome when the future Outcome contract exists; current single-owner work remains valid without requiring an Outcome migration.
- A **Run** is one Agent's execution attempt for a Task.
- A retry is a new Run linked to the same Task; it is never disguised as the same attempt.
- A delegated Run has a parent Task/Run and a separately bounded budget.
- Every Task has exactly one accountable primary Agent, even when later delegation is enabled.

### Event projection and Lease (target contracts, not implemented)

A canonical typed event log lets Web Ops, Telegram/Slack, CLI, Kanban and customer-generated UIs observe the same Outcome/Task/Run state through monotonic cursors. Projections apply actor, channel and data-scope grants; they never expose secrets or let a UI mutate the store directly.

A future multi-dispatcher claim may carry a Lease with an owner and expiry, heartbeat/renew and release/completion. Expiry marks work interrupted/unknown for explicit evaluation; it never automatically retries a model call, external write, payment or delivery. The current single-dispatcher lock does not gain a lease merely for presentation parity.

### Authority and integration independence (target contracts, not implemented)

Iskra becomes a hub only if it owns the canonical **Identity, Authority, Outcome, Evidence and Delivery** records. Protocol compatibility alone does not make Iskra independent. External Agents, Boards, models and tools are replaceable capabilities; external UIs are replaceable projections.

The target contracts are:

- `adapter/v1`: normalizes transport, external IDs, commands and events. Adapters never decide admission, permissions, acceptance, settlement or retry.
- `capability/v1`: provider-independent operations, artifacts, cancellation, health and declared resource limits. Unknown cost/health/capability remains `unknown`.
- `policy/v1`: issuer + tenant + agent instance + credential/reference + policy revision, with resource/operation/workspace/network/budget/expiry scopes. An external display profile is not a principal.
- `event/v1`: a CloudEvents-compatible envelope plus `outcome_id`, `task_id`, `run_id`, `delivery_id`, `actor`, `sequence`, `lease`, `scopes` and `policy_revision`.
- `evidence/v1`: artifact hash/provenance, acceptance result, evaluator, human correction/approval, total cost, latency, retries and resource use.

Integration invariants:

1. External input is untrusted material, never an owner instruction or a grant. Apply instruction hierarchy and content quarantine.
2. Delegated Agents never inherit broader secrets, workspace, network or budget than the parent approved.
3. Iskra's store is canonical; external stores are projections or caches, not active-active authorities.
4. `source_event_id`, `origin_adapter`, `forwarded_by` and `hop_count` prevent sync loops and duplicate Runs.
5. Adapter removal, vendor outage or account expiry must not destroy Iskra policy, evidence, Procedures or local built-in execution.
6. Lease expiry, provider timeout, delivery `unknown` and restart never automatically repeat side effects.
7. Export and restore cover Outcome, Evidence, Procedure, registry and audit data so exit does not depend on a vendor.

AgentField, CodeAF, OpenClaw and other authority runtimes already provide adjacent identity, policy, approval, headless or audit capabilities. Iskra must therefore prove its smaller operational form and distinct Outcome/BYOUI/failure semantics rather than claim those categories as unique.

### Workspace

A Workspace is an owner-approved file root identified by ID. Models never choose arbitrary host paths.

- `main.workspace` retains the existing `workspace/` root.
- Additional agents receive independent sibling roots under `agent-workspaces/<agent-id>/`.
- One Agent's root must not contain another Agent's root.
- A future owner-approved external project root requires an explicit registry entry and canonical-path validation.
- Cross-agent file exchange goes through the core Artifact Broker, not through a shared writable directory by default.

## 6. Component boundaries

```text
SaaS / Other Agents / Telegram / CLI / Web / generated UIs
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

The Core API is the only entry point for accepting work, changing run state, granting approval, recording usage, and publishing artifacts. Channels normalize input and render output; they do not decide permissions or independently persist execution state. The future public `outcome/v1` contract accepts desired results and acceptance/evidence requirements; `headless/v1` exposes bounded Outcome commands and cursor-based authorized event projections. Task and Run APIs remain subordinate execution interfaces.

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
- A Session or RunStore stop-write failure is projected as `StorageFailed`: the requested work is still aborted for safety, further admission and dispatch are poisoned, and no clean, persisted cancellation is reported.
- Only the expected current Run ID and event sequence may advance state; late duplicate events are harmless observations.
- `completed` means the core received a valid final result. It does not guarantee Telegram or network delivery.
- No state transition automatically authorizes a retry, payment, external send, or new Agent.

### Delivery projection (`delivery/v1`)

Delivery is a separate Core-owned projection. `memory/deliveries.json` is versioned, strictly validated, atomically written, private (`0600` on Unix), and bounded. It records delivery identity, Run and Agent identity, channel and owner-facing target, per-chunk state, attempt count, optional Telegram message id, and revision. It references the Run's redacted final answer in Session; it does not duplicate that answer into `runs.json` or the delivery store. The store is global to the data directory because one owner-facing channel may display answers from several Agents.

```text
pending -> sent              only after the channel returns a message id
pending -> failed            only when a rejection proves the message was not accepted
pending -> unknown           timeout, connection loss, ambiguous provider/API failure, or restart with an unresolved attempt
failed  -> pending           only as an explicit owner resend
unknown -> sent | failed     only through channel-specific reconciliation that proves the outcome
```

Rules:

- Persist `pending` before the external send. If that initial write fails, do not send.
- Persist `sent` only with the returned message id. A later write failure never causes a resend or a second Task execution.
- A Markdown parse rejection that the channel definitively refused may receive one plain-text fallback for the same logical delivery. Any other failure must not be retried automatically.
- `unknown` is not retried blindly, including after restart. A restart maps every unresolved `pending` record to `unknown`; it never sends, settles, or reruns work automatically.
- Each chunk is tracked. A partially sent answer is reported as partially sent plus the unresolved chunk count; it is not reported as fully delivered.
- Delivery is retried only by an explicit owner action and as a new recorded attempt. A fresh or explicitly resumed Run uses the same projection; resume never re-delivers an interrupted Run.
- `/status [@agent]` reports `pending`, `failed`, and `unknown` deliveries separately from Run execution state. `/result <run-id>` reads a completed Run's stored redacted answer without a model or tool call and, for Telegram, records its own explicit delivery. The existing private-chat and owner gates apply.
- RuntimeEvents are hints, not delivery authority. Missing, lagged, or closed terminal events are recovered from the durable Run, Session, and delivery records without leaving a watcher or typing indicator stuck.
- An invalid or unsupported delivery store is never reset or silently replaced. It fails closed for new sends and delivery-dependent admission, preserves the original file, and reports a localized storage error.
- If bounded retention cannot make room without discarding unresolved `pending`, `failed`, or `unknown` state, refuse new sends and report the condition rather than silently deleting delivery history.

## 8. Queue, concurrency, and routing

### Queue semantics

- One live dispatcher owns a data directory. Startup takes a non-blocking `flock` on the persistent private `memory/dispatcher.lock` before constructing Agent runtimes or loading the Run store; a second process fails closed before any model, Telegram, Session, or RunStore mutation.
- Each Agent has one durable FIFO queue by default.
- A Run is claimed atomically so a crash, restart, or second dispatcher cannot dispatch the same queue item twice. The kernel releases the dispatcher lock when its owner exits or is killed, so no stale-file heuristic exists.
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
  deliveries.json                  # global delivery/v1 projection, version 1
  dispatcher.lock                  # persistent private single-dispatcher flock target
  agents/<agent-id>/
    MEMORY.md
    SOUL.md
    session.json
logs/
  agent.log                        # existing operational metadata
  runs/<run-id>.ndjson             # bounded event metadata, when implemented
```

All user-specific registry, workspace, memory, run, and log paths remain ignored by Git. Registry and Run stores use private atomic writes. A missing registry means the compatibility `main` Agent synthesized from the existing `LLM_*` configuration; no file is created merely to answer a read-only status request.

A missing delivery store means no recorded deliveries and is created atomically on the first accepted send; its absence never creates work or sends a message. Delivery records are pruned only after bound and retention rules preserve unresolved `pending`, `failed`, and `unknown` records. No `session/v1` or `runs.json` schema change is part of Stage 2.5-A.

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

The implementation is split into three sequential, machine-verifiable goals: registry/runtime isolation, durable Run/Queue execution, then channel routing and localization. The following contracts are part of the approved Stage 2 design.

#### Registry and isolated AgentRuntime

The local `iskra.json` registry is strict `version: 1` JSON with only `engines` and `agents`. The public `iskra.example.json` contains credential-free examples only.

```json
{
  "version": 1,
  "engines": [
    {
      "id": "main-engine",
      "kind": "openai_compatible",
      "base_url": "https://api.deepseek.com/chat/completions",
      "model": "deepseek-flash",
      "api_key_env": "LLM_API_KEY"
    }
  ],
  "agents": [
    {
      "id": "main",
      "display_name": "Iskra",
      "engine_id": "main-engine",
      "role": "Personal assistant"
    }
  ]
}
```

- A missing registry synthesizes only `main` from the existing `LLM_*` / `DEEPSEEK_API_KEY` configuration. Read-only `/agents` never creates a registry.
- A present registry contains exactly one `main` and, during this slice, at most one additional Agent. Agents may reference the same Engine profile, but their runtime instances remain independent.
- Agent IDs start with a lowercase ASCII letter, contain only `a-z`, digits, and hyphens, and are at most 32 characters. Duplicate IDs, unknown fields/versions/Engine references, invalid URLs, empty models, and path collisions invalidate the registry and stop runtime construction.
- `api_key_env` stores an environment-variable name, never a credential. URL userinfo is refused. A missing `main` credential retains the existing startup error. A missing additional-Agent credential makes only that Agent `unavailable`; it never selects another model or provider as a silent fallback.
- Additional-Agent workspace and memory paths are derived by the core from validated IDs rather than accepted as arbitrary registry paths. During Stage 2 they are `agent-workspaces/<agent-id>/` and `memory/agents/<agent-id>/`: every component is created without following symlinks, each root is canonicalized, and a symlink, an existing non-directory, a target outside the approved derived location, or a root that equals or nests with another Agent root stops startup before any Session recovery write. Stage 2 keeps the existing core tool policy for both Agents. External roots and custom per-Agent tool policies remain later gates.
- An `AgentRuntime` owns its Agent/Engine IDs, `Brain`, `Tools`, `Identity`, `Session`, and `InstancePaths`. A `RuntimeRegistry` owns `main` and the optional additional Agent. CLI and channel composition use this registry instead of independently assembling authority-bearing parts.
- `loop_::run_turn` receives the selected Agent identity and core-issued Run ID rather than hard-coding `main`.

The independent state layout is:

```text
iskra.json                         # ignored, no secrets
iskra.example.json                 # public template
workspace/                         # existing main only
agent-workspaces/<agent-id>/       # additional Agent only
memory/
  MEMORY.md                        # existing main long-term memory
  session.json                     # existing main session/v1, unchanged
  stickies.md / board_state.json   # existing main only
  runs.json                        # global Run/Queue authority, version 1
  deliveries.json                  # global delivery/v1 projection, version 1
  dispatcher.lock                  # persistent private single-dispatcher flock target
  agents/<agent-id>/
    SOUL.md                        # registry role is the missing-file fallback
    MEMORY.md
    session.json                   # new Agent, existing session/v1 format
    stickies.md / board_state.json
```

`main` keeps its current SOUL/USER/memory/session/board paths and formats. An additional Agent reads its own SOUL plus the shared owner `USER.md`; it does not copy the main SOUL or MEMORY. `/iskra.json` and `/agent-workspaces/` are Git-ignored. Existing session data is not migrated or merged.

Every `model_call` metadata log includes `agent`, `run`, `engine`, and `model`. Legacy lines without an Agent field count as `main`. Usage accounting supports both the existing global total and `today_for_agent(agent_id)`. Agent self-state and targeted status use only the selected Agent's usage. Failure metadata follows the same rule: `model_error`, `balance_error`, and `tool_error` record Agent/Run/Engine/model where available, an error category and HTTP status where available, and the tool name, but never a provider body, prompt, response, tool argument, raw tool error, tool output, or credential.

#### RunStore authority and Session projection

`memory/runs.json` is the sole execution-state authority. Each Agent's `session.json` remains the conversation-replay and work-result projection; queue fields are not added to `session/v1`.

The strict, atomically written Run store contains:

- `version: 1`, a globally monotonic `next_run_id`, and a bounded `runs` list.
- Per Run: `id`, `agent_id`, redacted `input`, `state`, optional structured `failure`, optional `session_work_id`, and a `revision` for stale-event rejection.
- No final answer, reasoning, tool call, or tool output duplication; those conversation artifacts stay in Session according to its existing rules.

Credential substitution happens at admission. The resulting canonical input is used for both execution and Session persistence, so a restarted queued Run cannot change meaning. Inputs over 8,000 characters are refused rather than silently truncated. Each Agent has at most 20 unfinished queued Runs, and the store retains at most 100 Runs, pruning oldest terminal records first and never pruning queued, active, or paused records. Invalid UTF-8/JSON, unknown versions/fields, impossible transitions, and invalid Agent references preserve the original file and stop the runtime.

Stage 2 uses this reachable state machine:

```text
queued -> preparing -> running -> completed
    |         |           |-----> failed
    |         |           |-----> cancelled
    |         |-----------------> cancelled
    |         +-----------------> interrupted (restart recovery)
    |----------------------------> interrupted (restart recovery)
    +-> cancelled
    +-> queued_paused              (restart recovery)
queued_paused -> queued            (explicit /resume only)
```

`waiting_approval` remains inactive until an Approval Broker exists. There is no automatic-retry transition.

- **Enqueue:** persist `queued` before acknowledging acceptance or publishing an event. A write failure refuses the request before model/tool execution.
- **Claim:** under the Supervisor lock, persist the oldest eligible Agent queue head as `preparing` and reserve its global and per-Agent slots. No execution occurs before the write succeeds.
- **Dispatch:** create the Session work with `session.begin`, persist `preparing -> running`, and only then permit the first model/tool action.
- **Completion:** persist the Session result first, then the Run terminal state, then publish the terminal event. Any later write failure poisons the Run store against further dispatch and never triggers an automatic retry.
- **Events:** Core validates `run_id + revision` and emits monotonic per-Run sequence numbers. Late events after cancellation cannot reopen or complete a Run. Channels project `RuntimeEvent` values and do not persist completion themselves. Response text exists only in the in-memory event, not in the Run log.

#### FIFO, admission, recovery, and cancellation

- Per-Agent active concurrency is 1; global `preparing/running` concurrency is 2.
- The dispatcher considers only the oldest runnable head for each Agent and chooses the eligible head with the lowest global Run ID. Later work for the same Agent cannot overtake its head.
- Startup reads and validates the registry first, then acquires the dispatcher lock, and only then constructs Agent runtimes and opens the Run store. Lock-before-recovery matters because `Session::load` and `Core::open` perform recovery writes; a lock conflict performs none of them. A lock conflict exits nonzero with a clear English error and no model, Telegram, Session, or RunStore mutation.
- After the lock is held, startup changes `queued` to `queued_paused` and `preparing/running` to `interrupted`, with no automatic dispatch. Sessions retain their existing `running -> interrupted` recovery.
- A paused head blocks later work for that Agent. `/resume @agent` changes all of that Agent's paused queue entries back to `queued` in their original order. An interrupted Run itself is never re-executed.
- `/stop` targets `main`; `/stop @agent` targets the named Agent. One Run-store write cancels that Agent's active, queued, and paused work before Session stop is persisted and the active handle is aborted.
- If cancellation persistence fails, report the error and poison the runtime. Requested work is still stopped for safety, no replacement work is dispatched, affected watchers terminate with `StorageFailed`, and the channel does not claim a clean persisted stop. A Session or RunStore stop-write failure projects the same failure; later admission is refused until the store is valid again.
- Queue overflow returns localized backpressure before acceptance; it never silently drops a request.

#### Channel and command contract

- Only a leading `@agent ` in directly entered text selects an Agent. Plain text goes to `main`; `@main` is valid. Unknown, unavailable, malformed, or empty routes fail before a model call and never fall back as a security bypass.
- Forwarded text, stickers, and sticky-board replies keep their existing main-only behavior. Every non-numeric board reply, including one that begins with `@agent`, goes to `main`; a numeric reply removes a sticky. An `@agent` string inside forwarded material is not a route.
- Replies from a non-`main` Agent start with `@<agent_id>` on the first line; `main` replies stay unlabeled. The label is presentation-only, is added before Telegram's 4,096-character split, and leaves CLI output and stored Session/Run text unchanged.
- `/agents` lists Agent ID/display name, Engine/model, availability, and active/queued/paused counts without external balance or model requests.
- `/status` retains main status and adds Run/Queue information. `/status @agent` shows only the selected Agent's model, memory, Session, usage/cost, balance, and Run/Queue state.
- `/stop [@agent]` implements the cancellation contract. `/resume [@agent]` defaults to `main` and resumes only paused queued work. Resumed work delivers every terminal result to the requesting channel: Telegram subscribes before resume admission and reuses the per-Run watcher without a duplicate queue notice, and the CLI prints each resumed answer in FIFO order.
- `/start`, `/help`, the Telegram command menu, and queue acceptance/start/completion/failure/cancellation messages include the new behavior in `en/ko/ru`.
- CLI uses the same Core Agent-selection API and supports `@agent`, `/agents`, `/status [@agent]`, `/stop [@agent]`, and `/resume [@agent]` so real-process tests can exercise the vertical slice.

### Stage 2.5 — delivery reliability before further expansion

Delivery reliability is a prerequisite for external engines because an ACP turn is another indeterminate side-effect boundary. The stage is sequenced as delivery, then bounded context/resources, then execution-outcome uncertainty. Stage 2.5-A is complete; later slices require their own cards and measurements.

After Stage 2.5-A's owner-run gate, no later Stage starts automatically. The next code card follows the transaction and repeated-paid-demand gates in `docs/BUSINESS.md` §0 and §6. A fixed-scope Agent Replay & Delivery Audit uses the existing CLI/Run/Session/Delivery path and does not require a future Outcome or Headless API.

#### Stage 2.5-A — `delivery/v1`

- Add the Core-owned delivery projection from §7 and the local store from §9. Preserve the existing `session/v1`, `runs.json`, and registry formats.
- Classify Telegram results without changing the Task: persist before send; require a message id for `sent`; allow one plain-text fallback only for a definitive Markdown parse rejection; persist ambiguous outcomes as `unknown`; never auto-retry.
- Recover terminal and delivery state from durable records when events are missing or lagged, without rerunning work or leaving typing/watchers stuck.
- Add `/status [@agent]` delivery counts and `/result <run-id>` as the owner-only, model-free way to read a completed Run's stored answer. Telegram explicitly sends a tracked delivery; CLI prints the stored answer.
- Apply the existing en/ko/ru and Agent-attribution rules before chunk tracking.
- Do not change dependencies, on-disk Session/Run schemas, provider selection, approvals, context budgets, tool policy, or ACP.
- The delivery store is the authority for delivery, not Session or the channel. A missing file creates nothing by itself; corrupt/unsupported data fails closed without reset. Retention is bounded and never prunes unresolved records without an explicit owner action.

Stage 2.5-A acceptance requires automated transition, failure, restart, event-lag, chunking, localization, and real CLI process coverage; zero secret/prompt/tool-argument leakage; full formatting, clippy, release-build, and budget gates; and a separate owner-run Telegram check. A passing mock Bot API test is not live proof.

### Stage 3 — ACP and bounded delegation behind a demonstrated-need gate

- Implement one validated ACP path behind core approval and sandbox boundaries only when the built-in HTTP Agent cannot complete work for which actual payment has been validated.
- If delegation is separately justified and approved, use depth 1, bounded child count/time/tokens, and one accountable primary Agent.

### Stage 4 — Outcome, Headless and product surfaces behind separate gates

The Stage numbers name design layers, not authorization to implement them in sequence. Outcome, Headless, BYOUI and Managed are not one mandatory MVP bundle.

- After the commercial productization gate, define only the minimum `outcome/v1` and deterministic `evidence/v1` fields needed by one repeatedly requested scenario and one Outcome Pack. Do not pre-build three packs or split all six future contracts merely for symmetry.
- Add provider profiles and labelled cross-provider/cross-engine evaluations only when that selected scenario requires them. GLM is an early compatibility candidate, not a hard-coded product identity.
- Add a local authenticated `headless/v1` with idempotency, strict schemas, request bounds and cursor-based authorized event projections only when an actual external client must submit or observe Outcomes. It is a separate card from Outcome/Evidence.
- Add `adapter/v1`, `capability/v1`, `policy/v1`, `event/v1`, `verification/v1`, reusable Procedures/Routines, credential-free portable packages and Intake flows only as separately justified cards rather than one upfront abstraction program.
- Implement an official thin Outcome Board as one BYOUI projection only after a real interface need. Generated UIs run outside Core and begin read-only; approvals use Core commands only.
- Add managed always-on, schedules/webhooks, backup/restore and remote access only after local reliability and retention are demonstrated. Commercial Managed validation requires actual payment, not proposal acceptance.
- Treat Vercel Board as an optional future communication adapter rather than cloning its message store. Add leases only for an actual multi-dispatcher/Agent claim requirement.
- Evaluate rules and decision-model routing only against explicit/default baselines.

## 13. Verification requirements

Every code stage runs the full existing test suite, real CLI process tests, changed-file rustfmt, clippy, release build, and resource measurements. No completion claim relies only on compilation or an import.

Stage 1 must prove unchanged current behavior, including replay, reasoning echo, stop recovery, tool sandboxing, and usage recording.

Stage 2 must additionally prove:

- Every allowed and forbidden Run transition.
- FIFO order, atomic claim/settle, concurrency limits, and cancellation races.
- A single live dispatcher per data directory: a second real process exits before any model call or store write, and a later process acquires the lock after normal exit or owner kill.
- No model/tool request after restart until the owner explicitly resumes.
- Different Agents use their configured model and isolated memory/session/workspace.
- One Agent cannot read, write, advertise tools for, or charge usage to another Agent.
- Corrupt registry/state fails closed without resetting data.
- Logs and status distinguish Agent/Run/Engine/model and do not contain secrets or prompt contents. Provider- and tool-failure canary tests assert that unique prompt, provider-body, and tool-argument strings from a real CLI process never appear in `logs/agent.log`.
- Existing `session/v1` data replays as `main` history, legacy `model_call` usage is attributed to `main`, new Runs carry their own Agent attribution, and no history is merged into another Agent.
- Real owner-run Telegram and configured-provider checks pass before the feature is presented as live-verified.

Stage 2.5-A must prove:

- Every allowed and forbidden delivery transition, including `unknown` with no automatic retry and no automatic send after restart.
- A persisted pre-send record; no Telegram call when the initial delivery write fails; `sent` only with a message id; one plain-text fallback only for a definitive Markdown rejection.
- Chunk-level truthful state for partial sends, Agent labels, and `/status` counts.
- Terminal recovery after missing, lagged, or closed RuntimeEvents without rerunning a Task, duplicating a send, or leaving typing active.
- `/result <run-id>` returns a completed Run's stored redacted answer with zero model/tool calls, and explicit Telegram delivery is tracked.
- Corrupt, invalid-UTF-8, unknown-version, and unknown-field delivery data fails closed while preserving the original file; private atomic writes and bounded retention hold.
- No response text, prompt, tool argument, tool output, or credential is duplicated into the delivery store or logs; existing Session/Run data and schemas are unchanged.
- Real CLI process tests, formatting, clippy, release build, and all resource budgets pass. Owner-run Telegram delivery and rendering remain a separate live gate.

The ACP gate must prove approval allow/deny, unsupported-capability refusal, process-group cleanup, external file/network restrictions, and no automatic side-effect replay with both a mock ACP process and one real installed CLI.

The future Outcome/Headless/BYOUI gate must additionally prove:

- An Outcome cannot become `achieved` from Task/Run completion alone; declared acceptance and required evidence are mandatory.
- Local-only default bind, owner authentication, idempotency, strict schema/version, request/time/size bounds and fail-closed invalid state.
- Two independent clients receive identical authorized Outcome state through monotonic event cursors, with no automatic restart replay or duplicate admission.
- A generated/external UI cannot write stores, bypass approval, read another scope or export secrets. External sync uses minimum fields and warns about differing platform grants.
- Lease heartbeat, renew, release and expiry never automatically repeat side effects or reopen cancelled work.
- Duplicate/looped external events cannot create a new Run or side effect; adapter removal and vendor outage preserve canonical state and local built-in execution.
- The same Outcome Contract runs against two providers/engines without changing acceptance or evidence formats.
- A documented AgentField/CodeAF/OpenClaw comparison measures installation time, total resources/support, Outcome semantics, BYOUI scope enforcement, failure replay, provider swap, exit portability and managed recovery cost; no generic Hub claim is made without a practical advantage.
- Verified Outcome Rate, total cost, time to evidence, human intervention and delivery assurance are measured with external-engine resources included rather than hidden.

## 14. Reuse and license boundary

OpenMausBot's non-`enterprise/` code is Apache 2.0 and may be selectively ported with its license, copyright, NOTICE, and modification obligations. Any actual port records the source repository, exact commit, source files, destination files, and modifications. The `enterprise/` tree, trademarks, and mascot are excluded.

OpenBot is PolyForm Noncommercial 1.0.0. Iskra independently implements general ideas such as independent workspaces and durable queues; it does not copy OpenBot code, text, tests, or assets into the commercial product.

Vercel Board is a product/architecture reference for BYOUI, canonical event projections, claims and leases. Iskra independently designs its Outcome-first local contracts and does not copy Vercel branding, media, UI assets or implementation. Any future Board adapter or code reuse requires separate API, terms and license review. Board's documented US-only storage, broad default visibility, profile permissions and sync-spill limits are treated as requirements for stronger customer-selected locality and scoped projections, not as claims that every competing product lacks those controls.

References:

- https://github.com/milind-soni/OpenMausBot
- https://github.com/milind-soni/OpenMausBot/blob/main/LICENSING.md
- https://github.com/milind-soni/OpenMausBot/blob/main/server/contracts.ts
- https://github.com/milind-soni/OpenMausBot/blob/main/server/channel-queue.ts
- https://github.com/nightly-labs/openbot
- https://github.com/nightly-labs/openbot/blob/main/LICENSE
- https://agentclientprotocol.com/get-started/introduction
- https://vercel.com/blog/introducing-vercel-board
- https://vercel.com/docs/board
- https://www.anthropic.com/engineering/managed-agents
- https://docs.langchain.com/oss/python/langgraph/overview
- https://learn.microsoft.com/en-us/agent-framework/overview/
- https://temporal.io/solutions/ai
- https://modelcontextprotocol.io/docs/getting-started/intro
- https://a2a-protocol.org/latest/
- https://docs.z.ai/devpack/tool/claude
- https://agentfield.ai/docs/agentfield
- https://agentfield.ai/docs/codeaf
- https://openclaw.ai/
- https://docs.openclaw.ai/install
- https://hermes-agent.nousresearch.com/
