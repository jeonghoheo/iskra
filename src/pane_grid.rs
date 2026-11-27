//! Production `PaneGrid` seam over the `vt100` terminal model.
//!
//! The seam is deliberately small: feed bytes, resize, read a snapshot (text,
//! cursor, mode bits, scrollback), inspect cells, tap OSC/CSI callbacks and
//! page through scrollback. A pane host calls this adapter; the pane-host card
//! owns PTY lifetime and the `empty → spawning → live → interrupted → finished`
//! state, so nothing here spawns a process.
//!
//! The module is intentionally unreferenced until that card wires it. Keep the
//! single allowance below module-wide instead of scattering per-item
//! allowances, and remove it when the pane host calls the adapter.
#![allow(dead_code)]

use std::num::NonZeroU16;

/// An operating-system or control-sequence event observed while feeding bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OscEvent {
    /// An OSC payload that `vt100` does not interpret, for example Herdr's
    /// agent-status OSC 7501 or an application-specific OSC 9999.
    Unknown(Vec<u8>),
    /// OSC 2 window title.
    WindowTitle(Vec<u8>),
    /// OSC 1 icon name.
    IconName(Vec<u8>),
    /// OSC 52 clipboard write, encoded as `type;payload`.
    ClipboardCopy(Vec<u8>),
    /// OSC 52 clipboard read request.
    ClipboardPaste(Vec<u8>),
    /// Audible bell.
    Bell,
    /// Visual bell.
    VisualBell,
    /// A CSI sequence that `vt100` did not interpret.
    UnhandledCsi(String),
}

/// One terminal cell's text and wide-character placement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellInfo {
    /// Cell contents, empty for a wide-character continuation cell.
    pub text: String,
    /// First half of a double-width character.
    pub wide: bool,
    /// Second half of a double-width character.
    pub wide_cont: bool,
}

/// xterm mouse reporting mode currently enabled by the application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    /// Mouse reporting disabled.
    #[default]
    None,
    /// X10 press-only reporting.
    Press,
    /// Press and release reporting.
    PressRelease,
    /// Press, release and button-motion reporting.
    ButtonMotion,
    /// All motion reporting.
    AnyMotion,
}

/// Cursor shape requested by the application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// Terminal default.
    #[default]
    Default,
    /// Blinking block.
    BlinkBlock,
    /// Steady block.
    SteadyBlock,
    /// Blinking underline.
    BlinkUnderline,
    /// Steady underline.
    SteadyUnderline,
    /// Blinking vertical bar.
    BlinkVerticalBar,
    /// Steady vertical bar.
    SteadyVerticalBar,
}

/// A point-in-time view of one pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub rows: u16,
    pub cols: u16,
    /// `(row, column)` of the cursor.
    pub cursor: (u16, u16),
    pub hide_cursor: bool,
    pub alternate_screen: bool,
    pub bracketed_paste: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub mouse_mode: MouseMode,
    pub cursor_shape: CursorShape,
    pub scrollback_len: usize,
    pub scrollback_capacity: usize,
    /// Visible screen only, one entry per row, trailing blanks trimmed.
    pub lines: Vec<String>,
}

impl Snapshot {
    /// Visible screen as newline-joined text.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Replaceable terminal-grid backend.
pub trait PaneGrid {
    /// Feed raw terminal bytes. Partial UTF-8 sequences may span calls.
    fn process(&mut self, bytes: &[u8]);
    /// Resize the screen; the pane host must ask the application to redraw.
    fn resize(&mut self, rows: u16, cols: u16);
    /// Read the visible screen and mode bits.
    fn snapshot(&self) -> Snapshot;
    /// Inspect one cell, or `None` when outside the screen.
    fn cell(&self, row: u16, col: u16) -> Option<CellInfo>;
    /// Events observed so far, in arrival order.
    fn osc_events(&self) -> Vec<OscEvent>;
    /// Scroll back `n` rows and return up to `n + rows` lines in global order.
    fn scrollback_view(&mut self, n: usize) -> Vec<String>;
}

/// Construct the production backend. A later card may swap this factory.
pub fn new_grid(rows: u16, cols: u16, scrollback: usize) -> Box<dyn PaneGrid> {
    Box::new(Vt100Grid::new(rows, cols, scrollback))
}

#[derive(Default)]
struct Tap {
    events: Vec<OscEvent>,
}

impl vt100::Callbacks for Tap {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.events.push(OscEvent::Bell);
    }

    fn visual_bell(&mut self, _: &mut vt100::Screen) {
        self.events.push(OscEvent::VisualBell);
    }

    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, icon_name: &[u8]) {
        self.events.push(OscEvent::IconName(icon_name.to_vec()));
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.events.push(OscEvent::WindowTitle(title.to_vec()));
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        let mut v = ty.to_vec();
        v.push(b';');
        v.extend_from_slice(data);
        self.events.push(OscEvent::ClipboardCopy(v));
    }

    fn paste_from_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8]) {
        self.events.push(OscEvent::ClipboardPaste(ty.to_vec()));
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        self.events.push(OscEvent::Unknown(params.join(&b';')));
    }

    fn unhandled_csi(
        &mut self,
        _: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let mut s = String::new();
        if let Some(b) = i1 {
            s.push(b as char);
        }
        if let Some(b) = i2 {
            s.push(b as char);
        }
        s.push('[');
        s.push_str(
            &params
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .collect::<Vec<_>>()
                .join(";"),
        );
        s.push(c);
        self.events.push(OscEvent::UnhandledCsi(s));
    }
}

/// `vt100` implementation of [`PaneGrid`].
pub struct Vt100Grid {
    parser: vt100::Parser<Tap>,
}

impl Vt100Grid {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        let rows = NonZeroU16::new(rows.max(1)).expect("rows > 0");
        let cols = NonZeroU16::new(cols.max(1)).expect("cols > 0");
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, cols, scrollback, Tap::default()),
        }
    }
}

impl PaneGrid for Vt100Grid {
    fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let rows = NonZeroU16::new(rows.max(1)).expect("rows > 0");
        let cols = NonZeroU16::new(cols.max(1)).expect("cols > 0");
        self.parser.set_size(rows, cols);
    }

    fn snapshot(&self) -> Snapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let lines = screen
            .rows(0, cols.get())
            .map(|row| row.trim_end().to_string())
            .collect();
        Snapshot {
            rows: rows.get(),
            cols: cols.get(),
            cursor: screen.cursor_position(),
            hide_cursor: screen.hide_cursor(),
            alternate_screen: screen.alternate_screen(),
            bracketed_paste: screen.bracketed_paste(),
            application_cursor: screen.application_cursor(),
            application_keypad: screen.application_keypad(),
            mouse_mode: mouse_mode(screen.mouse_protocol_mode()),
            cursor_shape: cursor_shape(screen.cursor_style()),
            scrollback_len: screen.scrollback_len(),
            scrollback_capacity: screen.scrollback_capacity(),
            lines,
        }
    }

    fn cell(&self, row: u16, col: u16) -> Option<CellInfo> {
        self.parser.screen().cell(row, col).map(|c| CellInfo {
            text: c.contents().to_string(),
            wide: c.is_wide(),
            wide_cont: c.is_wide_continuation(),
        })
    }

    fn osc_events(&self) -> Vec<OscEvent> {
        self.parser.callbacks().events.clone()
    }

    fn scrollback_view(&mut self, n: usize) -> Vec<String> {
        let (rows, cols) = {
            let (r, c) = self.parser.screen().size();
            (r.get() as usize, c.get() as usize)
        };
        let len = self.parser.screen().scrollback_len();
        let n = n.min(len);
        // vt100 exposes scrollback as a viewport offset: each read returns one
        // screenful. Page through it and place every row in global order.
        let mut slots: Vec<Option<String>> = vec![None; len + rows];
        let mut off = 0usize;
        loop {
            self.parser.screen_mut().set_scrollback(off);
            let page: Vec<String> = self
                .parser
                .screen()
                .rows(0, cols as u16)
                .map(|r| r.trim_end().to_string())
                .collect();
            let start = len - off;
            for (i, line) in page.iter().enumerate() {
                let global = start + i;
                if global < slots.len() && slots[global].is_none() {
                    slots[global] = Some(line.clone());
                }
            }
            if off == n {
                break;
            }
            off = (off + rows).min(n);
        }
        self.parser.screen_mut().set_scrollback(0);
        slots
            .into_iter()
            .skip(len - n)
            .take(n + rows)
            .map(|s| s.unwrap_or_default())
            .collect()
    }
}

fn mouse_mode(mode: vt100::MouseProtocolMode) -> MouseMode {
    match mode {
        vt100::MouseProtocolMode::None => MouseMode::None,
        vt100::MouseProtocolMode::Press => MouseMode::Press,
        vt100::MouseProtocolMode::PressRelease => MouseMode::PressRelease,
        vt100::MouseProtocolMode::ButtonMotion => MouseMode::ButtonMotion,
        vt100::MouseProtocolMode::AnyMotion => MouseMode::AnyMotion,
    }
}

fn cursor_shape(style: vt100::CursorStyle) -> CursorShape {
    match style {
        vt100::CursorStyle::Default => CursorShape::Default,
        vt100::CursorStyle::BlinkBlock => CursorShape::BlinkBlock,
        vt100::CursorStyle::SteadyBlock => CursorShape::SteadyBlock,
        vt100::CursorStyle::BlinkUnderline => CursorShape::BlinkUnderline,
        vt100::CursorStyle::SteadyUnderline => CursorShape::SteadyUnderline,
        vt100::CursorStyle::BlinkVerticalBar => CursorShape::BlinkVerticalBar,
        vt100::CursorStyle::SteadyVerticalBar => CursorShape::SteadyVerticalBar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_osc_is_captured_without_grid_text() {
        let mut grid = new_grid(8, 60, 20);
        grid.process(
            b"A\x1b]7501;working;run=7\x07B\x1b]9999;xyz\x1b\\C\x1b]2;iskra-title\x07D\x1b]52;c;aGVsbG8=\x07E\x1b[99;99zF",
        );

        let events = grid.osc_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OscEvent::Unknown(v) if v.starts_with(b"7501;"))),
            "OSC 7501 must be observable: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OscEvent::Unknown(v) if v.starts_with(b"9999;"))),
            "OSC 9999 must be observable: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OscEvent::WindowTitle(v) if v == b"iskra-title")),
            "window title callback must fire: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OscEvent::ClipboardCopy(v) if v == b"c;aGVsbG8=")),
            "clipboard callback must fire: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OscEvent::UnhandledCsi(_))),
            "unknown CSI must be observable: {events:?}"
        );

        let snap = grid.snapshot();
        assert_eq!(snap.lines.first().map(String::as_str), Some("ABCDEF"));
        assert!(!snap.text().contains("7501"));
        assert!(!snap.text().contains("9999"));
    }

    #[test]
    fn icon_name_and_bells_are_reported() {
        let mut grid = new_grid(4, 20, 0);
        grid.process(b"\x1b]1;iskra-icon\x07\x07");

        let events = grid.osc_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OscEvent::IconName(v) if v == b"iskra-icon")),
            "icon name callback must fire: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, OscEvent::Bell)),
            "audible bell must be reported: {events:?}"
        );
    }

    #[test]
    fn mode_bits_round_trip() {
        let mut grid = new_grid(5, 20, 0);
        grid.process(b"\x1b[?1049h\x1b[?2004h\x1b[?1h\x1b=\x1b[?25l\x1b[?1000h\x1b[4 q");

        let set = grid.snapshot();
        assert!(set.alternate_screen);
        assert!(set.bracketed_paste);
        assert!(set.application_cursor);
        assert!(set.application_keypad);
        assert!(set.hide_cursor);
        assert_eq!(set.mouse_mode, MouseMode::PressRelease);
        assert_eq!(set.cursor_shape, CursorShape::SteadyUnderline);

        grid.process(b"\x1b[?1049l\x1b[?2004l\x1b[?1l\x1b>\x1b[?25h\x1b[?1000l\x1b[0 q");

        let reset = grid.snapshot();
        assert!(!reset.alternate_screen);
        assert!(!reset.bracketed_paste);
        assert!(!reset.application_cursor);
        assert!(!reset.application_keypad);
        assert!(!reset.hide_cursor);
        assert_eq!(reset.mouse_mode, MouseMode::None);
        assert_eq!(reset.cursor_shape, CursorShape::Default);
    }

    #[test]
    fn split_utf8_across_process_calls_is_held() {
        let mut grid = new_grid(2, 10, 0);

        grid.process(&[0xEA]);
        let after_first = grid.snapshot().text();
        assert!(!after_first.contains('\u{fffd}'));

        grid.process(&[0xB0, 0x80]);
        let cell = grid.cell(0, 0).expect("first cell");
        assert_eq!(cell.text, "가");
        assert!(cell.wide);
        assert!(!grid.snapshot().text().contains('\u{fffd}'));
    }

    #[test]
    fn wide_character_at_last_column_wraps() {
        let mut grid = new_grid(3, 5, 0);
        grid.process("abcd가".as_bytes());

        assert!(grid.snapshot().text().contains('가'));
        let first = grid.cell(0, 4).expect("last column of row 0");
        assert!(!first.wide, "a wide cell must not start in the last column");
        let second = grid.cell(1, 0).expect("first column of row 1");
        assert_eq!(second.text, "가");
        assert!(second.wide);

        for col in 0..5 {
            if let Some(cell) = grid.cell(1, col) {
                assert!(
                    !(cell.wide && col == 4),
                    "wide cell started at the last column of row 1"
                );
            }
        }
    }

    #[test]
    fn invalid_byte_does_not_corrupt_the_grid() {
        let mut grid = new_grid(2, 10, 0);
        grid.process(b"ok");
        grid.process(&[0xFF]);
        assert!(grid.snapshot().text().starts_with("ok"));
    }

    #[test]
    fn snapshot_reports_rows_cursor_and_scrollback_capacity() {
        let mut grid = new_grid(5, 20, 200);
        grid.process(b"hello");

        let snap = grid.snapshot();
        assert_eq!(snap.rows, 5);
        assert_eq!(snap.cols, 20);
        assert_eq!(snap.cursor, (0, 5));
        assert_eq!(snap.lines.len(), 5);
        assert_eq!(snap.lines[0], "hello");
        assert_eq!(snap.scrollback_len, 0);
        assert_eq!(snap.scrollback_capacity, 200);
    }

    #[test]
    fn resize_updates_the_reported_size() {
        let mut grid = new_grid(5, 20, 0);
        grid.resize(10, 40);

        let snap = grid.snapshot();
        assert_eq!((snap.rows, snap.cols), (10, 40));
        assert!(snap.lines.iter().all(|line| line.chars().count() <= 40));
    }

    #[test]
    fn scrollback_pages_in_global_order() {
        let mut grid = new_grid(24, 20, 200);
        for i in 0..224 {
            grid.process(format!("L{i:03}\r\n").as_bytes());
        }

        let snap = grid.snapshot();
        assert_eq!(snap.scrollback_len, 200);
        assert_eq!(snap.scrollback_capacity, 200);

        let view = grid.scrollback_view(200);
        assert_eq!(view.len(), 224);

        let numbers: Vec<usize> = view
            .iter()
            .filter_map(|line| line.strip_prefix('L'))
            .filter_map(|digits| digits.parse().ok())
            .collect();
        assert_eq!(
            numbers,
            (1..224).collect::<Vec<_>>(),
            "scrollback must be contiguous and oldest-first"
        );
        assert_eq!(view[view.len() - 2], "L223");
    }
}
