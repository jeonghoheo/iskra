---
name: goal-card
description: Draft an Iskra compact task card and machine-verifiable Goal-mode contract from approved specifications
type: prompt
whenToUse: When the user explicitly invokes this skill to prepare an Iskra implementation Goal
disableModelInvocation: true
arguments:
  - task
---

Prepare a draft for the requested Iskra task: $task

This procedure is read-only. Do not edit code or documents, create or resume a Goal, commit, push, contact Telegram or a real provider, or claim owner-run verification. Explain the decision in Korean, but write the task card and Goal contract in English following the repository's existing format.

1. Read `docs/WORKFLOW.md` §4–6 and the current task/status in `SESSION.md`. Read only the sections of `docs/ARCHITECTURE.md` referenced by that task, plus the current Iskra status in `/Users/heojeongho/Desktop/My/AI-personal-memory/NOW.md`. Do not restart broad architecture planning when the approved documents already resolve the design.
2. Inspect the actual branch, HEAD, working tree, and latest relevant recorded test baseline. Preserve all existing changes. Do not run tests, builds, lint, or resource measurements merely to create a draft; cite the latest recorded baseline and label it with its actual date/status. Verification runs only when the user separately requests it. If `$task` names a completed historical Goal, use its recorded card as the comparison baseline and label the output as a validation draft, not new work.
3. Apply the suitability gate before drafting:
   - The task has one implementation topic and an approved specification.
   - Its finish line is observable through automated commands, files, tests, process state, or other machine-readable evidence.
   - Allowed and forbidden paths, a bounded failure path, and stop conditions can be stated explicitly.
   - No unresolved design choice, owner input, real account, Telegram interaction, manual restart, external send, commit, or push is needed to finish.
   If any condition fails, do not produce a Goal contract. Return the smallest unresolved decision and recommend Plan mode or normal mode instead.
4. When suitable, output:
   - A compact task card with exactly these fields: `Outcome`, `Spec`, `Files`, `Proof`, `Current`, `Stop`, `Live`.
   - `Files` must name allowed and forbidden paths. `Proof` must name actual commands and exact passing results without inventing a test count. `Current` must distinguish confirmed measurements from older baselines. `Live` must keep owner verification outside the Goal.
   - A copy-ready Goal contract following `docs/WORKFLOW.md` §5, including `Done when`, `Scope`, `Loop`, `Stop`, and `Live`. Refer to exact specification sections rather than copying the architecture.
5. End with `누락·충돌`: list any missing evidence or contradictions, or write `없음`. Do not silently resolve a conflict between documents.
