# VT spike harness (2026-11-27)

Gate-form feasibility spike for the Iskra terminal-ownership direction
(`SESSION.md` → "Terminal ownership direction and Herdr analysis",
`docs/research/herdr-analysis-2026-11-27.md` §5).

This crate is **isolated on purpose**:

- It is not a workspace member and not part of the `iskra` package, so the
  production dependency count and stripped-size budget are unchanged until the
  gate passes and the owner separately approves landing `vt100`.
- `--no-default-features` builds the same harness with a `NullGrid` instead of
  the vt100 adapter. The stripped-size delta between the two builds is the
  measured cost of the candidate backend, not an estimate.
- No credential, personal file, network endpoint, or Iskra runtime data is
  touched. The PTY runs `/bin/bash`, `/usr/bin/vim`, and `/usr/bin/top` inside a
  temporary directory with an allowlisted environment.

## Run

```sh
cargo run                 # G0–G8, prints PASS/FAIL/STRUCTURAL + evidence
cargo run -- --only G2    # one gate while iterating
cargo run -- --json evidence.json
cargo run -- --probe 1 10 # one live bash pane held for 10s (resource probe)
./measure.sh              # stripped size delta + base/1-pane/4-pane RSS and fds
```

Exit code is 0 only when the gate passes.

## Seam

`src/grid.rs` defines `PaneGrid` (feed bytes, resize, snapshot, cell, OSC
events, paged scrollback) and the vt100 adapter. The snapshot carries the mode
bits that the Herdr failure catalog says belong in the pane contract:
alternate screen, bracketed paste, application cursor/keypad, hidden cursor,
mouse protocol mode, cursor style.

`src/pty.rs` spawns a program on a PTY (`openpty` + `setsid` + `TIOCSCTTY`),
supports resize and raw input, and reads non-blocking output. macOS-only ioctl
constants are defined locally because `libc` does not export them for Apple.

## Gates

| Gate | Scored | Check |
|---|---|---|
| G0 | no | OSC 7501/9999/title/clipboard are observable and never reach the grid text |
| G1 | yes | bash prompt renders through the seam at 24×80 |
| G2 | yes | vim enters the alternate screen, shows typed text, saves, and exits |
| G3 | yes | `top` produces at least two distinct frames and a real `PID` header |
| G4 | yes | Hangul occupies two cells (base + continuation), cursor advances by display width |
| G5 | yes | grid resize keeps width invariants; the PTY reports the new size to the app |
| G6 | yes | 200 scrollback rows are readable in order (paged viewport reads) |
| G7 | no | CR submit, arrow-key history, UTF-8 input, Ctrl-C interrupt |
| G8 | no | split UTF-8, wide-char boundary, invalid byte, mode-bit round trip, memory math |

Five of the six scored gates must pass. A `STRUCTURAL` result, or the same
approach failing twice, stops the spike and returns options instead of
implementation.
