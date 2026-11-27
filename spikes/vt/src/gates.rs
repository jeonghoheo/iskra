//! Gate G0–G8 for the terminal-ownership VT spike.
//!
//! Every gate is machine-checkable and reports PASS / FAIL / STRUCTURAL with
//! evidence. The six scored gates are G1–G6; the spike proceeds at 5/6.

use crate::grid::{new_grid, OscEvent, PaneGrid, Snapshot};
use crate::pty::Pty;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Structural,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Structural => "STRUCTURAL",
        }
    }
}

pub struct GateResult {
    pub id: &'static str,
    pub name: &'static str,
    pub scored: bool,
    pub status: Status,
    pub evidence: Vec<String>,
}

pub struct Report {
    pub gates: Vec<GateResult>,
    pub measurements: Vec<(String, String)>,
}

impl Report {
    pub fn scored_pass(&self) -> usize {
        self.gates.iter().filter(|g| g.scored && g.status == Status::Pass).count()
    }
    pub fn scored_total(&self) -> usize {
        self.gates.iter().filter(|g| g.scored).count()
    }
    pub fn structural(&self) -> bool {
        self.gates.iter().any(|g| g.status == Status::Structural)
    }
    pub fn verdict(&self) -> &'static str {
        if self.structural() {
            "STOP — structural finding, present options"
        } else if self.scored_pass() >= self.scored_total().min(5) {
            "PROCEED — 5/6 gate satisfied"
        } else {
            "STOP — budget infeasible at this gate"
        }
    }
    pub fn ok(&self) -> bool {
        !self.structural() && self.scored_pass() >= self.scored_total().min(5)
    }

    pub fn print(&self) {
        println!("== VT spike: gate results ==");
        for g in &self.gates {
            println!(
                "{:<3} {:<32} {:<10} {}",
                g.id,
                g.name,
                g.status.as_str(),
                if g.scored { "scored" } else { "support" }
            );
            for e in &g.evidence {
                println!("      {e}");
            }
        }
        println!();
        for (k, v) in &self.measurements {
            println!("measure {} = {}", k, v);
        }
        println!();
        println!(
            "SCORE scored_pass={}/{}",
            self.scored_pass(),
            self.scored_total()
        );
        println!("VERDICT {}", self.verdict());
    }

    pub fn to_json(&self) -> String {
        let mut s = String::from("{\n  \"gates\": [\n");
        for (i, g) in self.gates.iter().enumerate() {
            s.push_str("    {\"id\": \"");
            s.push_str(g.id);
            s.push_str("\", \"name\": \"");
            s.push_str(g.name);
            s.push_str("\", \"scored\": ");
            s.push_str(if g.scored { "true" } else { "false" });
            s.push_str(", \"status\": \"");
            s.push_str(g.status.as_str());
            s.push_str("\", \"evidence\": [");
            for (j, e) in g.evidence.iter().enumerate() {
                if j > 0 {
                    s.push_str(", ");
                }
                s.push('"');
                s.push_str(&json_escape(e));
                s.push('"');
            }
            s.push_str("]}");
            if i + 1 < self.gates.len() {
                s.push(',');
            }
            s.push('\n');
        }
        s.push_str("  ],\n  \"measurements\": {");
        for (i, (k, v)) in self.measurements.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push('"');
            s.push_str(k);
            s.push_str("\": \"");
            s.push_str(&json_escape(v));
            s.push('"');
        }
        s.push_str("},\n  \"scored_pass\": ");
        s.push_str(&self.scored_pass().to_string());
        s.push_str(",\n  \"scored_total\": ");
        s.push_str(&self.scored_total().to_string());
        s.push_str(",\n  \"verdict\": \"");
        s.push_str(&json_escape(self.verdict()));
        s.push_str("\"\n}\n");
        s
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn gate(id: &'static str, name: &'static str, scored: bool, f: impl FnOnce() -> GateResult) -> GateResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(mut r) => {
            r.id = id;
            r.name = name;
            r.scored = scored;
            r
        }
        Err(_) => GateResult {
            id,
            name,
            scored,
            status: Status::Fail,
            evidence: vec!["panic while running gate".into()],
        },
    }
}

fn pass(id: &'static str, name: &'static str, scored: bool, evidence: Vec<String>) -> GateResult {
    GateResult {
        id,
        name,
        scored,
        status: Status::Pass,
        evidence,
    }
}

fn fail(id: &'static str, name: &'static str, scored: bool, evidence: Vec<String>) -> GateResult {
    GateResult {
        id,
        name,
        scored,
        status: Status::Fail,
        evidence,
    }
}

fn workdir() -> PathBuf {
    let d = std::env::temp_dir().join("iskra-vt-spike");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    hay.matches(needle).count()
}

fn last_nonempty(lines: &[String]) -> &str {
    lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|s| s.as_str())
        .unwrap_or("")
}

fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

struct Live {
    pty: Pty,
    grid: Box<dyn PaneGrid>,
}

impl Live {
    fn start(rows: u16, cols: u16, scrollback: usize) -> io::Result<Live> {
        let pty = Pty::spawn(
            &["/bin/bash", "--norc", "--noprofile", "-i"],
            rows,
            cols,
            &workdir(),
        )?;
        Ok(Live {
            pty,
            grid: new_grid(rows, cols, scrollback),
        })
    }

    fn pump(&mut self, ms: i32) -> io::Result<bool> {
        let mut got = false;
        // Drain quickly; poll once for the requested idle window afterwards.
        for _ in 0..64 {
            match self.pty.read(0)? {
                Some(b) => {
                    self.grid.process(&b);
                    got = true;
                }
                None => break,
            }
        }
        if !got {
            if let Some(b) = self.pty.read(ms)? {
                self.grid.process(&b);
                got = true;
            }
        }
        Ok(got)
    }

    fn send(&mut self, s: &str) -> io::Result<()> {
        self.pty.write_all(s.as_bytes(), Duration::from_secs(2))
    }

    fn snap(&self) -> Snapshot {
        self.grid.snapshot()
    }

    fn text(&self) -> String {
        self.grid.snapshot().text()
    }

    fn wait_until<F: FnMut(&Live) -> bool>(&mut self, timeout_ms: u64, mut f: F) -> bool {
        let start = Instant::now();
        loop {
            if f(self) {
                return true;
            }
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                return false;
            }
            let _ = self.pump(50);
        }
    }

    fn wait_prompt(&mut self, marker: &str, timeout_ms: u64) -> bool {
        let m = marker.to_string();
        self.wait_until(timeout_ms, move |live| {
            let snap = live.snap();
            last_nonempty(&snap.lines).starts_with(&m)
        })
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.pty.resize(rows, cols);
        self.grid.resize(rows, cols);
    }
}

fn wide_splits(grid: &dyn PaneGrid, rows: u16, cols: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if let Some(cell) = grid.cell(r, c) {
                if cell.wide {
                    let ok = c + 1 < cols
                        && grid
                            .cell(r, c + 1)
                            .map(|n| n.wide_cont)
                            .unwrap_or(false);
                    if !ok {
                        out.push((r, c));
                    }
                }
            }
        }
    }
    out
}

pub fn run_all(only: Option<&str>) -> Report {
    let mut gates = vec![
        gate("G0", "unknown-OSC observability", false, g0_unknown_osc),
        gate("G1", "bash prompt through seam", true, g1_bash_prompt),
        gate("G2", "vim opens and exits", true, g2_vim),
        gate("G3", "top refreshes", true, g3_top),
        gate("G4", "Korean 2-cell alignment", true, g4_korean),
        gate("G5", "resize without corruption", true, g5_resize),
        gate("G6", "200-line scrollback read", true, g6_scrollback),
        gate("G7", "input encoding matrix", false, g7_input),
        gate("G8", "bounds and mode-bit tests", false, g8_bounds),
    ];
    if let Some(only) = only {
        gates.retain(|g| g.id == only);
    }

    Report {
        gates,
        measurements: Vec::new(),
    }
}

fn g0_unknown_osc() -> GateResult {
    let mut grid = new_grid(8, 60, 20);
    // Herdr agent status (unknown to vt100), an arbitrary unknown OSC, a
    // window title, a clipboard write, and an unknown CSI in between text.
    grid.process(
        b"A\x1b]7501;working;run=7\x07B\x1b]9999;xyz\x1b\\C\x1b]2;iskra-title\x07D\x1b]52;c;aGVsbG8=\x07E\x1b[99;99zF",
    );
    let snap = grid.snapshot();
    let events = grid.osc_events();

    let captured: Vec<String> = events
        .iter()
        .map(|e| match e {
            OscEvent::Unknown(v) => format!("unknown:{}", String::from_utf8_lossy(v)),
            OscEvent::WindowTitle(v) => format!("title:{}", String::from_utf8_lossy(v)),
            OscEvent::IconName(v) => format!("icon:{}", String::from_utf8_lossy(v)),
            OscEvent::ClipboardCopy(v) => format!("clip:{}", String::from_utf8_lossy(v)),
            OscEvent::ClipboardPaste(v) => format!("paste:{}", String::from_utf8_lossy(v)),
            OscEvent::Bell => "bell".to_string(),
            OscEvent::VisualBell => "visual-bell".to_string(),
            OscEvent::UnhandledCsi(v) => format!("csi:{}", v),
        })
        .collect();

    let visible_first = snap.lines.first().cloned().unwrap_or_default();
    let has_7501 = events
        .iter()
        .any(|e| matches!(e, OscEvent::Unknown(v) if v.starts_with(b"7501;")));
    let has_9999 = events
        .iter()
        .any(|e| matches!(e, OscEvent::Unknown(v) if v.starts_with(b"9999;")));
    let has_title = events
        .iter()
        .any(|e| matches!(e, OscEvent::WindowTitle(v) if v == b"iskra-title"));
    let visible_clean = visible_first == "ABCDEF";

    let mut evidence = vec![
        format!("visible_row0={:?}", visible_first),
        format!("events={:?}", captured),
        format!("osc_7501_observed={}", has_7501),
        format!("osc_9999_observed={}", has_9999),
        format!("window_title_observed={}", has_title),
        "no hook needed: vt100::Callbacks::unhandled_osc delivers unknown OSC payloads".into(),
    ];
    if has_7501 && has_9999 && has_title && visible_clean {
        pass("G0", "unknown-OSC observability", false, evidence)
    } else {
        evidence.push("unknown OSC payloads are not fully observable through the adapter".into());
        fail("G0", "unknown-OSC observability", false, evidence)
    }
}

fn g1_bash_prompt() -> GateResult {
    let mut live = match Live::start(24, 80, 200) {
        Ok(l) => l,
        Err(e) => return fail("G1", "bash prompt through seam", true, vec![format!("spawn: {e}")]),
    };
    let _ = live.send("PS1='ISKRA> '\r\n");
    let _ = live.send("clear\r\n");
    let prompt_ok = live.wait_prompt("ISKRA>", 6000);
    let _ = live.send("printf 'PROMPT_READY\\n'\r\n");
    let ready = live.wait_until(4000, |l| count(&l.text(), "PROMPT_READY") >= 2);
    let prompt_returned = live.wait_prompt("ISKRA>", 3000);
    let snap = live.snap();

    let mut evidence = vec![
        format!("size={}x{}", snap.rows, snap.cols),
        format!("prompt_last_line={:?}", last_nonempty(&snap.lines)),
        format!("prompt_seen={}", prompt_ok),
        format!("command_output_seen={}", ready),
        format!("prompt_returned_after_output={}", prompt_returned),
        format!("shell_alive={}", live.pty.try_wait() == Some(false)),
        format!("cursor={:?}", snap.cursor),
    ];
    let ok = prompt_ok && ready && prompt_returned && snap.rows == 24 && snap.cols == 80;
    if ok {
        pass("G1", "bash prompt through seam", true, evidence)
    } else {
        evidence.push(format!("screen=\n{}", snap.text()));
        fail("G1", "bash prompt through seam", true, evidence)
    }
}

fn g2_vim() -> GateResult {
    let file = workdir().join("vimfile.txt");
    let _ = std::fs::write(&file, "");
    let mut live = match Live::start(24, 80, 200) {
        Ok(l) => l,
        Err(e) => return fail("G2", "vim opens and exits", true, vec![format!("spawn: {e}")]),
    };
    let _ = live.send("PS1='V> '\r\nclear\r\n");
    let _ = live.wait_prompt("V>", 6000);
    let _ = live.send("vim -u NONE -N -n -i NONE vimfile.txt\r\n");

    let alt = live.wait_until(8000, |l| l.snap().alternate_screen);
    let drew = alt && live.wait_until(4000, |l| l.text().contains("vimfile.txt"));
    let _ = live.send("i");
    let _ = live.pump(150);
    let _ = live.send("hello from vt spike");
    let visible_before_exit = live.wait_until(2500, |l| l.text().contains("hello from vt spike"));
    let _ = live.send("\u{1b}");
    let _ = live.pump(250);
    let _ = live.send(":wq\r\n");
    let left_alt = live.wait_until(8000, |l| !l.snap().alternate_screen);
    let _ = live.wait_prompt("V>", 6000);
    let saved = std::fs::read_to_string(&file).unwrap_or_default();

    let mut evidence = vec![
        format!("entered_alt_screen={}", alt),
        format!("drew_text_on_alt_screen={}", drew),
        format!("typed_text_visible_before_exit={}", visible_before_exit),
        format!("left_alt_screen={}", left_alt),
        format!("file_after_exit={:?}", saved),
    ];
    if alt && drew && left_alt && saved.contains("hello from vt spike") {
        pass("G2", "vim opens and exits", true, evidence)
    } else {
        evidence.push(format!("screen=\n{}", live.text()));
        fail("G2", "vim opens and exits", true, evidence)
    }
}

fn g3_top() -> GateResult {
    let mut live = match Live::start(24, 80, 200) {
        Ok(l) => l,
        Err(e) => return fail("G3", "top refreshes", true, vec![format!("spawn: {e}")]),
    };
    let _ = live.send("PS1='T> '\r\nclear\r\n");
    let _ = live.wait_prompt("T>", 6000);
    let _ = live.send("top -l 2 -s 1 -n 5\r\n");

    let mut sample_line = String::new();
    let saw_header = live.wait_until(8000, |l| {
        let t = l.text();
        if let Some(line) = t.lines().find(|x| x.contains("PID")) {
            sample_line = line.to_string();
            true
        } else {
            false
        }
    });
    let mut hashes = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(6) {
        let _ = live.pump(150);
        let h = hash(&live.text());
        if hashes.last().copied() != Some(h) {
            hashes.push(h);
        }
        if hashes.len() >= 2 {
            break;
        }
    }
    let _ = live.send("\u{3}");
    let _ = live.pump(200);
    let _ = live.send("clear\r\n");
    let _ = live.wait_prompt("T>", 4000);
    let text = live.text();

    let mut evidence = vec![
        format!("header_seen={}", saw_header),
        format!("distinct_frames={}", hashes.len()),
        format!("sample_line={:?}", sample_line),
    ];
    if saw_header && hashes.len() >= 2 {
        pass("G3", "top refreshes", true, evidence)
    } else {
        evidence.push(format!("screen=\n{}", text));
        fail("G3", "top refreshes", true, evidence)
    }
}

fn g4_korean() -> GateResult {
    let mut grid = new_grid(4, 20, 0);
    grid.process("안녕하세요".as_bytes());
    let snap = grid.snapshot();

    let chars = ['안', '녕', '하', '세', '요'];
    let mut checks = Vec::new();
    let mut ok = true;
    for (i, ch) in chars.iter().enumerate() {
        let base = grid.cell(0, (i * 2) as u16);
        let cont = grid.cell(0, (i * 2 + 1) as u16);
        let base_ok = base
            .as_ref()
            .map(|c| c.wide && c.text == ch.to_string())
            .unwrap_or(false);
        let cont_ok = cont.as_ref().map(|c| c.wide_cont).unwrap_or(false);
        if !base_ok || !cont_ok {
            ok = false;
        }
        checks.push(format!(
            "col{}={:?}/wide={} cont_wide={}",
            i * 2,
            base.map(|c| c.text).unwrap_or_default(),
            base_ok,
            cont_ok
        ));
    }
    let cursor_ok = snap.cursor == (0, 10);
    let text_ok = snap.lines.first().map(|l| l.as_str()) == Some("안녕하세요");

    // Mixed ASCII + Hangul advances by display width, not char count.
    let mut mixed = new_grid(2, 20, 0);
    mixed.process("a한b".as_bytes());
    let mixed_cursor = mixed.snapshot().cursor;

    let mut evidence = vec![
        format!("cells={:?}", checks),
        format!("cursor={:?} expected=(0, 10)", snap.cursor),
        format!("text_match={}", text_ok),
        format!("mixed_cursor={:?} expected=(0, 4)", mixed_cursor),
    ];
    if ok && cursor_ok && text_ok && mixed_cursor == (0, 4) {
        pass("G4", "Korean 2-cell alignment", true, evidence)
    } else {
        evidence.push("wide-cell alignment is not preserved".into());
        fail("G4", "Korean 2-cell alignment", true, evidence)
    }
}

fn g5_resize() -> GateResult {
    // Grid-level resize.
    let mut grid = new_grid(24, 80, 100);
    for i in 0..40 {
        grid.process(format!("line-{:03}\r\n", i + 1).as_bytes());
    }
    grid.resize(12, 40);
    let snap = grid.snapshot();
    let too_wide = snap.lines.iter().filter(|l| l.chars().count() > 40).count();

    // Naive width truncation can split a wide cell; measure it explicitly.
    let mut wide = new_grid(3, 10, 0);
    wide.process("가나다라마바".as_bytes());
    wide.resize(3, 5);
    let splits_before_redraw = wide_splits(wide.as_ref(), 3, 5);

    // Live resize: the PTY must report the new size and the app must redraw.
    let mut live = match Live::start(24, 80, 200) {
        Ok(l) => l,
        Err(e) => {
            return fail(
                "G5",
                "resize without corruption",
                true,
                vec![format!("spawn: {e}")],
            )
        }
    };
    let _ = live.send("PS1='R> '\r\nclear\r\n");
    let _ = live.wait_prompt("R>", 6000);
    let _ = live.send("stty size\r\n");
    let saw_old = live.wait_until(4000, |l| {
        l.text().lines().any(|x| x.trim() == "24 80")
    });
    live.resize(30, 100);
    let _ = live.pump(300);
    let _ = live.send("stty size\r\n");
    let saw_new = live.wait_until(5000, |l| {
        l.text().lines().any(|x| x.trim() == "30 100")
    });
    let _ = live.send("printf 'KOREAN %s\\n' 한글한글한글한글한글한글한글한글한글한글\r\n");
    let _ = live.pump(400);
    let live_snap = live.snap();
    let splits_after_redraw = wide_splits(live.grid.as_ref(), live_snap.rows, live_snap.cols);

    let mut evidence = vec![
        format!("grid_resized={}x{}", snap.rows, snap.cols),
        format!("rows_over_new_width={}", too_wide),
        format!("wide_split_cells_after_shrink_stale_grid={}", splits_before_redraw.len()),
        format!("pty_reported_old_size={}", saw_old),
        format!("pty_reported_new_size={}", saw_new),
        format!("wide_split_cells_after_live_redraw={}", splits_after_redraw.len()),
    ];
    let ok = snap.rows == 12
        && snap.cols == 40
        && too_wide == 0
        && saw_old
        && saw_new
        && splits_after_redraw.is_empty();
    if ok {
        if !splits_before_redraw.is_empty() {
            evidence.push(
                "caveat: vt100 width resize truncates naively and can leave a split wide cell in a stale grid; the pane host must force an app redraw on resize (bash/top/vim do)".into(),
            );
        }
        pass("G5", "resize without corruption", true, evidence)
    } else {
        evidence.push(format!("screen=\n{}", live.text()));
        fail("G5", "resize without corruption", true, evidence)
    }
}

fn g6_scrollback() -> GateResult {
    let mut grid = new_grid(24, 80, 200);
    for i in 1..=300 {
        grid.process(format!("line-{:03}\r\n", i).as_bytes());
    }
    let snap = grid.snapshot();
    let view = grid.scrollback_view(200);
    let view_len = view.len();
    let first = view.first().cloned().unwrap_or_default();
    let last = last_nonempty(&view).to_string();
    let line_rows: Vec<usize> = view
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("line-"))
        .map(|(i, _)| i)
        .collect();
    let contiguous = line_rows.windows(2).all(|w| w[1] == w[0] + 1)
        && view
            .iter()
            .filter(|l| l.starts_with("line-"))
            .enumerate()
            .all(|(i, l)| l.trim() == format!("line-{:03}", first_num(&first) + i));
    let has_100 = view.iter().any(|l| l.trim() == "line-100");
    let has_200 = view.iter().any(|l| l.trim() == "line-200");

    let mut evidence = vec![
        format!("capacity={}", snap.scrollback_capacity),
        format!("scrollback_len={}", snap.scrollback_len),
        format!("view_lines={} expected={}", view_len, snap.scrollback_len + 24),
        format!("first={:?} last={:?}", first, last),
        format!("contiguous={} has_line100={} has_line200={}", contiguous, has_100, has_200),
    ];
    let ok = snap.scrollback_capacity == 200
        && snap.scrollback_len == 200
        && view_len == snap.scrollback_len + 24
        && first.starts_with("line-0")
        && last == "line-300"
        && contiguous
        && has_100
        && has_200;
    if ok {
        pass("G6", "200-line scrollback read", true, evidence)
    } else {
        evidence.push("scrollback is not retrievable in order".into());
        fail("G6", "200-line scrollback read", true, evidence)
    }
}

fn first_num(line: &str) -> usize {
    line.trim()
        .strip_prefix("line-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn g7_input() -> GateResult {
    let mut live = match Live::start(24, 80, 200) {
        Ok(l) => l,
        Err(e) => {
            return fail(
                "G7",
                "input encoding matrix",
                false,
                vec![format!("spawn: {e}")],
            )
        }
    };
    let _ = live.send("PS1='I> '\r\nclear\r\n");
    let _ = live.wait_prompt("I>", 6000);

    // CR submits; output appears twice (echo + result).
    let _ = live.send("echo HIST_ALPHA\r\n");
    let cr_ok = live.wait_until(4000, |l| count(&l.text(), "HIST_ALPHA") >= 2);

    // Up arrow (CSI A) must recall history and run it again.
    let _ = live.send("\u{1b}[A");
    let _ = live.pump(250);
    let _ = live.send("\r\n");
    let arrow_ok = live.wait_until(4000, |l| count(&l.text(), "HIST_ALPHA") >= 4);

    // UTF-8 input reaches the program as one character.
    let _ = live.send("printf 'KOR_%s\\n' 한글\r\n");
    let utf8_ok = live.wait_until(4000, |l| l.text().contains("KOR_한글"));

    // Ctrl-C interrupts a foreground process: wait for the command echo first,
    // then interrupt, then prove the shell accepts a new command.
    let _ = live.send("sleep 30\r\n");
    let submitted = live.wait_until(4000, |l| l.text().contains("sleep 30"));
    let _ = live.pump(400);
    let before_prompt = count(&live.text(), "I>");
    let _ = live.send("\u{3}");
    let _ = live.pump(500);
    let prompt_back = live.wait_until(3000, move |l| {
        count(&l.text(), "I>") > before_prompt
    });
    let _ = live.send("echo AFTER_INT\r\n");
    let int_ok = submitted
        && prompt_back
        && live.wait_until(4000, |l| count(&l.text(), "AFTER_INT") >= 2);

    let mut evidence = vec![
        format!("cr_submit={}", cr_ok),
        format!("arrow_history_recall={}", arrow_ok),
        format!("utf8_input_roundtrip={}", utf8_ok),
        format!("ctrl_c_interrupt={}", int_ok),
        format!("interrupt_submitted={} prompt_back={}", submitted, prompt_back),
    ];
    if cr_ok && arrow_ok && utf8_ok && int_ok {
        pass("G7", "input encoding matrix", false, evidence)
    } else {
        evidence.push(format!("screen=\n{}", live.text()));
        fail("G7", "input encoding matrix", false, evidence)
    }
}

fn g8_bounds() -> GateResult {
    let mut evidence = Vec::new();
    let mut ok = true;

    // 1) Split UTF-8 across process() calls must not produce a replacement.
    let mut split = new_grid(2, 10, 0);
    split.process(&[0xEA]); // first byte of U+AC00
    let after_first = split.snapshot().text();
    split.process(&[0xB0, 0x80]);
    let cell = split.cell(0, 0);
    let split_ok = cell
        .as_ref()
        .map(|c| c.text == "가" && c.wide)
        .unwrap_or(false)
        && !split.snapshot().text().contains('\u{fffd}')
        && !after_first.contains('\u{fffd}');
    ok &= split_ok;
    evidence.push(format!(
        "split_utf8_ok={} text={:?}",
        split_ok,
        split.snapshot().text()
    ));

    // 2) A wide character at the last column must wrap, not split.
    let mut edge = new_grid(3, 5, 0);
    edge.process("abcd가".as_bytes());
    let splits = wide_splits(edge.as_ref(), 3, 5);
    let edge_ok = splits.is_empty() && edge.snapshot().text().contains("가");
    ok &= edge_ok;
    evidence.push(format!(
        "wide_at_boundary_wraps={} splits={:?}",
        edge_ok, splits
    ));

    // 3) Invalid UTF-8 must not panic or corrupt the grid.
    let mut invalid = new_grid(2, 10, 0);
    invalid.process(b"ok");
    invalid.process(&[0xFF]);
    let invalid_ok = invalid.snapshot().text().starts_with("ok");
    ok &= invalid_ok;
    evidence.push(format!(
        "invalid_byte_no_panic={} text={:?}",
        invalid_ok,
        invalid.snapshot().text()
    ));

    // 4) Mode bits are part of the snapshot contract and must round-trip.
    let mut modes = new_grid(5, 20, 0);
    modes.process(b"\x1b[?1049h\x1b[?2004h\x1b[?1h\x1b=\x1b[?25l\x1b[?1000h\x1b[4 q");
    let m1 = modes.snapshot();
    let modes_ok = m1.alternate_screen
        && m1.bracketed_paste
        && m1.application_cursor
        && m1.application_keypad
        && m1.hide_cursor
        && m1.mouse_mode != "None"
        && m1.cursor_style.contains("Underline");
    ok &= modes_ok;
    evidence.push(format!(
        "modes_set alt={} paste={} app_cursor={} app_keypad={} hide_cursor={} mouse={} style={}",
        m1.alternate_screen,
        m1.bracketed_paste,
        m1.application_cursor,
        m1.application_keypad,
        m1.hide_cursor,
        m1.mouse_mode,
        m1.cursor_style
    ));

    modes.process(b"\x1b[?1049l\x1b[?2004l\x1b[?1l\x1b>\x1b[?25h\x1b[?1000l");
    let m2 = modes.snapshot();
    let reset_ok = !m2.alternate_screen
        && !m2.bracketed_paste
        && !m2.application_cursor
        && !m2.application_keypad
        && !m2.hide_cursor
        && m2.mouse_mode == "None";
    ok &= reset_ok;
    evidence.push(format!(
        "modes_reset alt={} paste={} app_cursor={} app_keypad={} hide_cursor={} mouse={}",
        m2.alternate_screen,
        m2.bracketed_paste,
        m2.application_cursor,
        m2.application_keypad,
        m2.hide_cursor,
        m2.mouse_mode
    ));

    // 5) Grid memory footprint per scrollback row (budget input).
    let per_row_80 = 80usize * 32; // vt100 Cell is 32 bytes (compile-time assert)
    let per_row_200 = 200usize * 32;
    evidence.push(format!(
        "cell_bytes=32 scrollback_row_80col={}B scrollback_row_200col={}B rows_per_MiB_80col={} rows_per_MiB_200col={}",
        per_row_80,
        per_row_200,
        1_048_576 / per_row_80,
        1_048_576 / per_row_200
    ));

    if ok {
        pass("G8", "bounds and mode-bit tests", false, evidence)
    } else {
        fail("G8", "bounds and mode-bit tests", false, evidence)
    }
}

/// Resource probe: hold `panes` live bash panes and sleep.
pub fn probe(panes: usize, secs: u64) {
    let mut live = Vec::new();
    for _ in 0..panes {
        match Live::start(24, 80, 200) {
            Ok(mut l) => {
                let _ = l.send("PS1='P> '\r\nclear\r\n");
                let _ = l.wait_prompt("P>", 4000);
                live.push(l);
            }
            Err(e) => {
                eprintln!("probe spawn failed: {e}");
                std::process::exit(1);
            }
        }
    }
    println!(
        "probe_ready pid={} panes={} pane_pids={:?}",
        std::process::id(),
        panes,
        live.iter().map(|l| l.pty.pid()).collect::<Vec<_>>()
    );
    std::thread::sleep(Duration::from_secs(secs));
    let _ = Path::new("/tmp");
}
