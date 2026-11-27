//! `PaneGrid` seam plus the vt100 adapter.
//!
//! The seam is deliberately small: feed bytes, resize, read a snapshot (text,
//! cursor, mode bits, scrollback). A later Iskra implementation would put this
//! trait in `src/` and keep the adapter replaceable.

#[cfg(feature = "vt100")]
use std::num::NonZeroU16;

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // variants are unused only in the feature-off size baseline
pub enum OscEvent {
    Unknown(Vec<u8>),
    WindowTitle(Vec<u8>),
    IconName(Vec<u8>),
    ClipboardCopy(Vec<u8>),
    ClipboardPaste(Vec<u8>),
    Bell,
    VisualBell,
    UnhandledCsi(String),
}

#[derive(Clone, Debug)]
pub struct CellInfo {
    pub text: String,
    pub wide: bool,
    pub wide_cont: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub rows: u16,
    pub cols: u16,
    pub cursor: (u16, u16),
    pub hide_cursor: bool,
    pub alternate_screen: bool,
    pub bracketed_paste: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub mouse_mode: String,
    pub cursor_style: String,
    pub scrollback_len: usize,
    pub scrollback_capacity: usize,
    /// Visible screen only, one entry per row, trailing blanks trimmed.
    pub lines: Vec<String>,
}

impl Snapshot {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

pub trait PaneGrid {
    fn process(&mut self, bytes: &[u8]);
    fn resize(&mut self, rows: u16, cols: u16);
    fn snapshot(&self) -> Snapshot;
    fn cell(&self, row: u16, col: u16) -> Option<CellInfo>;
    fn osc_events(&self) -> Vec<OscEvent>;
    /// Scroll back `n` rows and return scrollback + screen contents.
    fn scrollback_view(&mut self, n: usize) -> Vec<String>;
}

pub fn new_grid(rows: u16, cols: u16, scrollback: usize) -> Box<dyn PaneGrid> {
    #[cfg(feature = "vt100")]
    {
        Box::new(Vt100Grid::new(rows, cols, scrollback))
    }
    #[cfg(not(feature = "vt100"))]
    {
        let _ = (rows, cols, scrollback);
        Box::new(NullGrid::default())
    }
}

#[cfg(feature = "vt100")]
#[derive(Default)]
struct Tap {
    events: Vec<OscEvent>,
}

#[cfg(feature = "vt100")]
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

#[cfg(feature = "vt100")]
pub struct Vt100Grid {
    parser: vt100::Parser<Tap>,
}

#[cfg(feature = "vt100")]
impl Vt100Grid {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        let rows = NonZeroU16::new(rows.max(1)).expect("rows > 0");
        let cols = NonZeroU16::new(cols.max(1)).expect("cols > 0");
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, cols, scrollback, Tap::default()),
        }
    }
}

#[cfg(feature = "vt100")]
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
            mouse_mode: format!("{:?}", screen.mouse_protocol_mode()),
            cursor_style: format!("{:?}", screen.cursor_style()),
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
                let g = start + i;
                if g < slots.len() && slots[g].is_none() {
                    slots[g] = Some(line.clone());
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

/// Feature-off baseline: identical harness, no terminal model. Used only to
/// measure the stripped-size cost of the vt100 adapter.
#[cfg(not(feature = "vt100"))]
#[derive(Default)]
pub struct NullGrid {
    bytes: usize,
}

#[cfg(not(feature = "vt100"))]
impl PaneGrid for NullGrid {
    fn process(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len();
    }
    fn resize(&mut self, _rows: u16, _cols: u16) {}
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            rows: 0,
            cols: 0,
            cursor: (0, 0),
            hide_cursor: false,
            alternate_screen: false,
            bracketed_paste: false,
            application_cursor: false,
            application_keypad: false,
            mouse_mode: String::new(),
            cursor_style: String::new(),
            scrollback_len: 0,
            scrollback_capacity: 0,
            lines: Vec::new(),
        }
    }
    fn cell(&self, _row: u16, _col: u16) -> Option<CellInfo> {
        None
    }
    fn osc_events(&self) -> Vec<OscEvent> {
        Vec::new()
    }
    fn scrollback_view(&mut self, _n: usize) -> Vec<String> {
        Vec::new()
    }
}
