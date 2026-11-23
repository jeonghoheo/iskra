# Iskra Development Workflow for Token and Time Efficiency

Status: **approved operational guide for planning and executing Iskra work**.
Date: 2026-11-23.

This guide assigns each kind of work to the cheapest mode that can prove it correctly. It does not replace repository reading, fail-closed safety, verification, or commit approval rules.

## 1. Core principle

Do not repeat planning merely because a task moves from design to implementation.

```text
Large design once
  -> compact task card
  -> autonomous DeepSeek goal when machine-verifiable
  -> owner verification in normal mode when human input is required
  -> stronger-model review only for new decisions or high-risk failures
```

The large specification remains the source of truth. A smaller Plan-mode document is needed only when the large specification leaves a real decision unresolved.

## 2. Mode selection

| Work | Mode and model | Reason |
|---|---|---|
| Architecture, external-project analysis, new security/business decisions | Plan mode + stronger model | Judgment and trade-offs matter more than token cost |
| A smaller step with an unresolved design choice | Plan mode + stronger model | Stop the wrong implementation before spending execution tokens |
| Implementation with known files and automated acceptance checks | Goal mode + DeepSeek | It can iterate and stop on objective proof |
| Short reads, log updates, wording corrections | Normal mode | Goal creation and audits cost more than the task |
| Telegram interaction, real-account checks, visual checks, manual restarts | Normal mode | Human input must not trigger repeated autonomous continuations |
| Two failed attempts, sandbox/authority changes, difficult root-cause analysis | Stronger-model diagnosis/review | Accurate diagnosis is cheaper than repeated failed implementation |

A goal is suitable only when its finish line can be observed through tests, commands, files, process state, or another machine-readable result.

## 3. Large design contract

Use Plan mode once for the broad architecture. Record:

- Outcome and current state.
- Stage boundaries and explicit non-goals.
- State transitions and security boundaries.
- Automated and owner-run verification for every Stage.
- The source-of-truth path and current Git baseline.
- Whether any downstream decision remains unresolved.

Keep the result in a durable repository document such as `docs/ARCHITECTURE.md`. Session-specific plan files and long conversations are working aids, not the sole source of truth.

Afterward, update only sections affected by new decisions. Do not regenerate the whole design for each implementation task.

## 4. Compact task card

Before each implementation goal, create a card of roughly ten lines under the relevant Stage in `SESSION.md` or another existing project tracking document.

```text
Outcome: What must become true
Spec: Exact design document and section
Files: Allowed and forbidden paths
Proof: Automated command and exact passing result
Current: Confirmed HEAD, test baseline, unresolved dependency
Stop: When to stop rather than widen scope
Live: Separate owner verification that remains outside the goal
```

Rules:

- If `Spec`, `Files`, `Proof`, and `Stop` are already clear, skip a smaller Plan-mode document.
- If a real choice remains, resolve only that choice in Plan mode.
- Refer to exact sections instead of embedding the full architecture in every goal.
- A new session reads the required repository/personal state documents once, then reads the current task card and relevant source. Continuations must not reread the same material without a change or contradiction.
- Keep one current Stage and one next goal in the personal `NOW.md`, so a new execution does not need the whole conversation history.

## 5. Goal contract

Use this structure:

```text
Implement <Stage/item> from <Spec>.
Done when <automated command> produces <exact result> and only <allowed paths> changed.
Scope: <allowed work>. Exclude <later stages, dependencies, external effects, commit/push>.
Loop: Run focused checks while iterating; run the full milestone verification once at the end.
Stop: On <missing decision, external input, two failed approaches>, record the actual blocker and stop.
Live: Do not claim <owner-only verification> from automated results.
```

A good goal has:

- One implementation topic.
- Existing automated verification.
- Explicit allowed and forbidden paths.
- A bounded failure path.
- No default token cap. Scope and stop conditions control cost.
- No pending owner input, account setup, or manual approval needed to finish.

If a required manual step is discovered after the goal starts, record it and stop. Do not leave autonomous continuations running while waiting for a person.

## 6. Human verification stays outside goals

Telegram checks, real-provider behavior, screen inspection, and other owner-driven steps use normal mode.

Prepare one complete checklist with:

- Exact commands and messages.
- Expected visible result.
- Log evidence to inspect.
- Cleanup actions.
- Failure reporting format.

The owner performs the checklist without repeated goal blocked/resume cycles. Record the actual result once afterward.

An observed interactive verification goal took 24 turns and 19.4k tokens, largely because owner-input waiting triggered autonomous continuations. This is an anti-pattern, not a target baseline for future work.

## 7. Iteration without waste

During implementation:

- Run the smallest relevant tests while diagnosing a failure.
- Run the full suite, real process checks, release build, and resource measurements once at the milestone, unless a new failure requires another pass.
- Do not rerun a passing check merely to restate success.
- Start long builds/tests in the background and perform independent review while they run.
- When a required background result is needed, use `WaitFor`; do not repeatedly poll status.
- Read targeted source ranges after the initial orientation instead of repeatedly loading full large files.
- Do not repeat completed web research unless freshness or a contradiction requires it.
- After every actual change, update stale comments and documentation, but do not create extra narrative reports.
- Keep progress messages short: the next non-obvious action or the newly discovered fact.

## 8. Failure and escalation card

After the same approach fails twice, stop that approach and record only:

```text
Symptom: actual failing command/result
Cause: confirmed cause, or explicitly labelled hypothesis
Change: files and behavior touched
Attempts: approaches already tried
Next: next smallest test or the decision requiring a stronger model/owner
```

Escalate security boundaries, data migration, provider protocol contradictions, and unresolved architectural choices. Do not escalate routine syntax or test failures merely because a stronger model is available.

## 9. Current example: Stage 1 decomposition

Do not put all of Stage 1 into one large goal. Use three sequential goals from `docs/ARCHITECTURE.md` §12–13.

### Goal 1 — Engine and RunContext contracts

- Stop `Brain` from directly calling the global tool schema function.
- Inject the approved tool schemas/executor and Run identity through an explicit context.
- Preserve request serialization, reasoning echo, usage, and model errors.
- Prove with existing brain/loop/session tests and focused mock HTTP requests.

### Goal 2 — Instance-owned Tools, Memory, and Session boundaries

- Reduce global workspace, capability advertisement, and storage-path coupling.
- Keep compatibility `main` on the existing `workspace/` and `memory/session.json`.
- Prove with existing tool sandbox, memory, session, and real CLI process tests.

### Goal 3 — RunSupervisor owns channel execution

- Move Telegram `JoinHandle` ownership out of the channel and into the Supervisor.
- Preserve `/stop`, restart recovery, localization, typing activity, and final replies.
- Prove with existing channel/session tests and real local Bot API checks.

Skip another Plan-mode pass if these boundaries remain unchanged. Return to Plan mode if a new security boundary, schema migration, or contract choice appears.

After all three goals pass their automated milestone checks, perform the owner-run Telegram/provider verification once in normal mode.

## 10. Completion and Git checkpoints

At the end of each goal record only:

- Changed files.
- Actual checks and counts.
- Unverified items.
- The single next action or blocker.

Do not commit or push automatically per goal. The owner approves commits at a Stage or another meaningful review checkpoint.

The workflow is successful when it reduces waiting turns, repeated reads, repeated passing checks, and rework—not merely when it uses a cheaper model.
