# Herdr analysis and Iskra terminal-ownership direction

Date: 2026-11-27. Observed versions: herdr TUI 0.9.5 (`e8d7d97`), herdr-core 0.1.22 (`2e8043a`), herdr-app (`1180a5c`), herdr-layout-examples (`0ec9676`); herdr.dev pages were fetched the same day. All Herdr repositories are Apache-2.0 and were read from a temporary clone only; no source, text, or asset is copied into Iskra (ARCHITECTURE §14).

Status: **owner-approved direction; no implementation.** This document supports the personal-memory `DECISIONS.md` entries of 2026-11-27 and the `SESSION.md` card of the same date.

## 1. Conclusion

Terminal ownership is mechanically feasible inside the existing budget and commercially unproven. Iskra should occupy the gap between terminal hosts (Herdr) and record/authority runtimes (AgentField-class): a local single-binary core that owns panes, keeps run/delivery/approval/evidence records as the canonical state, and serves both a terminal surface and the existing `headless/v1` API as projections of the same `runs.json`.

Herdr's own evidence shows the split: it owns PTY lifetime, layout, agent status (OSC 7501), delegation (`hc`), and headless clients (OpenRPC), but its canonical state is screen content plus program-reported status. It has no approval broker, acceptance evidence, or delivery receipts.

## 2. Terminal-ownership contract (Iskra proposal)

What survives a restart:

- Run/Session/Delivery/Approval/Queue records, layout, display metadata, event-ring cursors.
- Pane slots as `interrupted` state.

What does not survive:

- Pane processes. A crash marks panes `interrupted`; zero model, tool, or external calls run before an explicit owner action.

Panes: `empty → spawning → live → interrupted → finished`; transitions are core-only, unknown stays unknown. Failure modes: core crash, PTY write failure, fd exhaustion (admission refusal), corrupt settings (refuse, no fallback). Metrics: record consistency after restart, auto-replay count (must be 0), fds per pane, scrollback bytes per pane.

Herdr comparison: after a holder loss, kept terminals come back as a new shell with an explicit "-- the program above ended; this is a new shell --" marker [herdr-core `src/kept.rs`]; OSC 7577 lets a program opt into resume ("a report is never a launch by itself") [herdr-core `src/resume.rs`]; a lost conversation after a crash was the reported failure [herdr-core #324]. Iskra keeps that honesty and adds the record spine.

## 3. Surface ladder (L0–L3)

- **L0 data:** layout (BSP), view filter/sort (`all/any/not/eq/in/exists` over builtin and token fields), theme, keys, TTL'd metadata tokens (bounded sources and keys, sequence freshness, set/clear exclusion). Herdr precedent: `herdr/src/layout.rs`, `herdr/src/agent_view_eval.rs`, `herdr/src/metadata_tokens.rs`.
- **L1 expressions:** deterministic format/aggregate over snapshot fields only; no I/O, clock, randomness or host handles.
- **L2 pure scripts:** `fn(snapshot, event) -> view_tree | intent[]`; no I/O, clock, randomness or host handles; command/time/memory/snapshot caps; same input produces the same output; effects are applied by the host, approvals and transitions by core policy.
- **L3 argv plugins:** JSON manifest, argv arrays (no shell), platform gates, `ISKRA_PLUGIN_ROOT/CONFIG_DIR/STATE_DIR` env injection, default-empty capability set. Herdr precedent: `herdr/src/app/api/plugins/manifest.rs`, `env.rs`; herdr-app `herdr-shell/src/policy.rs`, `capability.rs`.

L0 covers: inbox grouping/filter/sort, layout, theme/keys, TTL token labels. Not L0: elapsed-time badges (L1 if the host exposes `working_ms`, otherwise L2), interactive approval widgets (L2/L3).

## 4. VT strategy

| Candidate | Version | Direct deps | License | Notes |
|---|---|---|---|---|
| vte | 0.15.0 (2025-02) | 2 (+optional) | Apache-2.0 OR MIT | parser only; grid, scrollback and reflow must be built |
| vt100 | 0.18.1 (2026-10-19) | 4 | MIT | full screen model and scrollback; single maintainer |
| alacritty_terminal | 0.26.0 (2026-04-06) | 20 | Apache-2.0 | production grid including reflow; heavy dependency tree |
| ghostty-vt vendored | pinned submodule | Zig 0.16 build | Apache-2.0 | Herdr's choice; Windows CI break [herdr-core #361]; rejected |

Recommendation: `vt100` behind a `PaneGrid` seam so the backend stays replaceable. Size estimates are unverified: vt100 roughly +300–600 KB stripped; alacritty_terminal roughly +1.5–3 MB.

## 5. VT spike (gate form; no calendar)

- G0: unknown-OSC observability.
- G1: seam plus vt100 adapter draws a bash prompt.
- G2–G7: vim opens and exits; top refreshes; Korean 2-cell alignment; resize without corruption; 200-line scrollback read.
- G8: half-character boundary and mode-bit snapshot tests; stripped size, RSS and fd measurements.

Each item is classified PASS / FAIL (reproducible cause) / STRUCTURAL. 5/6 pass → proceed. Two failures of the same approach or one STRUCTURAL finding → stop and present the next options.

## 6. Budget (owner-approved 2026-11-27)

Base RSS ≤ 30 MB with no panes; total ≤ 30 MB + 2 MB per pane (1 MB scrollback cap, ≤ 8 panes); stripped binary ≤ 20 MB; ≤ 15 direct dependencies.

Evidence: Herdr's default scrollback is 5 MB per terminal [herdr-core `src/config.rs`], and macOS soft NOFILE 256 exhausts at about 85 terminals with 3 fds each [herdr-core #110].

Measurement: `ps -o rss= -p <pid>`, `lsof -p <pid> | wc -l`, stripped release binary size.

## 7. Failure catalog

| Herdr failure | Lesson for Iskra |
|---|---|
| core #324 kept terminal returns as a plain shell | restart restores slots, not conversations; say so plainly |
| core #108 half-printed multibyte character at attach | hold split UTF-8 bytes across snapshot boundaries |
| core #110 fd exhaustion at ~85 terminals | raise rlimit, cap panes, document fds per pane |
| core #249 disk-full kills the registry writer thread | a log write failure must not kill the persistence path |
| core #162 holder adoption changes status without an event | every externally visible state change emits an event |
| herdr #6215 bracketed-paste mode unreachable | mode bits belong in the pane snapshot contract |
| herdr #6153 WSL Shift+Enter arrives as bare CR | input encoding matrix is a first-class test dimension |
| herdr #6149 reconnect drops a full PaneSurface | test 2+ clients on reconnect paths |
| herdr-app #1065 caller environment leaks into every pane | construct pane env from an allowlist, never inherit wholesale |
| herdr-core #361 Zig toolchain breaks the Windows build | avoid toolchain-coupled vendoring |

## 8. Falsification conditions (no durations)

1. VT: both candidates classify ≤ 4/6 PASS with STRUCTURAL failures → ownership is budget-infeasible; stop and present options.
2. Demand: the audit offer reaches 300 meaningful views with 0 paid orders and 0 external requests for pane surfaces → the surface/ownership track has no demand evidence.
3. Competition: Herdr ships receipts/approval/evidence export AND audit conversion stays below 1 per 100 views → the responsibility differentiator is not purchasable.

## 9. Unverified

- vt100 top, reflow and unknown-OSC behavior.
- Stripped-size impact of vt100 and alacritty_terminal.
- L2 engine choice (mlua or rquickjs) and whether in-process script isolation is sufficient.
- Whether Herdr's `report_agent` authority model can be fully reproduced without its holder split.
