---
name: milestone-check
description: Run Iskra's approved local milestone verification and resource measurements once, then report actual evidence
type: prompt
whenToUse: When the user explicitly invokes this skill at an Iskra implementation milestone
disableModelInvocation: true
---

Run the current Iskra milestone verification once. Do not edit product code, tests, documentation, or memory files; do not fix failures, commit, or push. This skill is not for focused iteration checks and must not be rerun merely to restate a passing result.

1. Read `docs/WORKFLOW.md` §7 and §10, the current milestone card and acceptance criteria in `SESSION.md`, and the latest relevant verification record in `SESSION-LOG.md`. The current card and approved workflow are the source of truth. Record the actual branch, HEAD, `git status --short`, and existing uncommitted changes before running checks. Preserve them all.
2. Run the full automated checks required by the current card, normally:
   - `cargo test`, reporting unit, integration, real-process, failed, and ignored counts from actual output.
   - `rustfmt --edition 2024 --check --config skip_children=true` on the Rust files changed by the current milestone. Do not impose repository-wide formatting where the approved record documents pre-existing debt; report any failure without reformatting.
   - `cargo clippy --all-targets`, separating actual new warnings from the latest recorded baseline. Do not hard-code the old warning count.
   - `cargo build --release`, distinguishing Iskra warnings from transitive dependency notices.
3. Measure the hard budgets without touching real credentials or personal data:
   - Copy `target/release/iskra` to a temporary directory, strip only the copy, measure its bytes with `stat -f%z`, calculate MiB, and remove the copy.
   - Read `Cargo.toml` and report the actual direct dependency names and count.
   - Measure CLI idle RSS after one second from an isolated temporary working directory using `--cli`, a dummy `LLM_API_KEY`, a loopback dummy `LLM_BASE_URL`, and a dummy model name. Explicitly remove inherited provider/Telegram credentials from the child environment. Keep stdin open without sending a model request, measure the actual Iskra process with `ps -o rss= -p <pid>`, stop it, close stdin, and remove the temporary directory. Do not read or copy the repository `.env`, `memory/`, `workspace/`, or `logs/`.
4. Compare the stripped binary, idle RSS, and dependency count with the hard limits in `/Users/heojeongho/Desktop/My/AI-personal-memory/AGENTS.md` and the current card. A budget excess or failed required check means the milestone failed; record the evidence and stop rather than changing code or weakening the criterion.
5. Report in Korean with `실행 환경`, `자동 검증`, `측정값`, `실패·미검증`, and `다음 행동`. Separate automated evidence from owner-run Telegram, real-provider, visual, and en/ru live checks. Include an English `SESSION-LOG.md` draft containing changed files, actual commands and counts, measurements, failures, unverified items, and one next action, but do not append it.
6. Finish by checking `git status --short` for unexpected tracked files or new unignored artifacts. Do not delete pre-existing files or ignore-rule-covered build/test output. Report anything whose origin is uncertain.
