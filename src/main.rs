//! dsh-lite — a very small DeepSeek "harness" TUI for the Cardputer Zero.
//!
//! The loop is a port of DSH's `core/agent-loop` + `tools` pipeline (see
//! `harness.rs` and `tool.rs`): a turn owns steps, a step is one request plus
//! its tool batch, and every call traverses
//! tool/call -> pre-execute -> execute -> post-execute -> tool/result with
//! `additionalContexts` injected after the batch.
//!
//! No sandbox, no subagents, no persistence: tools run as the invoking user.
//!
//! Zero external crates: JSON is hand-rolled, HTTP goes through `curl`.

mod api;
mod apps;
mod art;
mod artifact;
mod ask;
mod bg;
mod checkpoint;
mod cmd;
mod compact;
mod config;
mod dsh_login;
mod context;
mod goal;
mod fb;
mod harness;
mod hwinfo;
mod pet;
mod pet_art;
mod pet_growth;
mod pet_ui;
mod i18n;
mod instructions;
mod jobs;
mod login;
mod mcp;
mod sandbox;
mod session;
mod skills;
mod todo;
mod usage;
mod json;
mod tool;
mod tools;
mod update;
mod vision;
mod voice;

use api::Cfg;
use artifact::Artifact;
use harness::{Action, Ev};
use std::io::{Read, Write};

// ------------------------------------------------------------------ terminal

pub struct Scr {
    rows: usize,
    cols: usize,
    lines: Vec<(String, u8)>, // (text, kind)  kind: 0 normal 1 user 2 tool 3 toolres 4 err 5 dim
    scroll: usize,
    input: Vec<char>,
    cursor: usize,
    note: Option<String>,
    /// Messages typed while a turn was running, oldest first.
    queue: Vec<String>,
    /// Local images attached to the next human message (sent once, then
    /// rewritten to a text placeholder in the history).
    attachments: Vec<crate::vision::Attachment>,
    /// Ctrl-C/Ctrl-D asked the app to exit.
    quit: bool,
    /// A turn is running (drives the busy hint).
    busy: bool,
    /// Tab was pressed while busy: start recording once the turn ends.
    voice_pending: bool,
    /// A recorder already opened by the busy screen when Tab interrupted the
    /// turn: the mic is live before the turn unwinds, so no words are lost.
    pending_rec: Option<crate::voice::Recorder>,
    /// Whether the prompt was empty when push-to-talk started. An empty prompt
    /// plus auto-send means hands-free send; with text in the prompt the take is
    /// inserted as dictation (append at the cursor, never overwrite, no send).
    voice_start_empty: Option<bool>,
    /// Full-screen QR overlay (login).
    qr: Option<Vec<String>>,
    /// Whether speech (ASR/TTS) is available for the current endpoint.
    speech_ok: bool,
    /// Live reasoning lines (folded into one summary line when content starts).
    think_lines: Vec<String>,
    think_active: bool,
    /// Animated splash: deterministic frame scheduler, hold countdown and
    /// whether the animation is running. See `art::Splash`.
    art: art::Splash,
    art_hold: u16,
    art_on: bool,
    /// menuconfig-style configuration screen.
    menu: Option<Menu>,
    /// Selected row in the `/` command palette.
    palette_sel: usize,
    /// True once the QR has been blitted to the framebuffer: further terminal
    /// repaints would paint over it, so draw() becomes a no-op.
    qr_blit: bool,
    /// Context window size of the current model, for the ring widget.
    ctx_window: u64,
    /// `base|model` the cached ctx_window belongs to (avoids a /models probe
    /// on every turn just to keep the ring correct).
    ctx_for: String,
    /// Hide typed characters (API key entry).
    mask: bool,
    /// Let a bare Enter submit an empty line (prompts whose default is "keep
    /// what is configured", e.g. the onboarding model/base questions).
    allow_empty: bool,
    /// Submit automatically once speech fills the input.
    auto_send: bool,
    /// Incomplete UTF-8 sequence being assembled from keystrokes.
    utf8_buf: Vec<u8>,
    /// Full-screen panel currently taking over the display.
    artifact: Option<Artifact>,
    /// The opening splash has been scrolled away already.
    splash_done: bool,
    /// Live line above the prompt: (1 = thinking, 2 = streaming tail, text).
    live: Option<(u8, String)>,
    /// Progress line from the `state_report` tool, pinned to the top row until
    /// the next transcript line arrives.
    state_line: Option<String>,
    /// A finished background update check waiting for the user's yes/no.
    update_offer: Option<crate::update::UpdateInfo>,
    /// One-time version banner shown on startup until the first user action.
    startup_notice: Option<String>,
    /// Strict-mode permission pop-up.
    approval: Option<ApprovalView>,
    /// Interactive `ask_user` question modal.
    ask: Option<AskView>,
    /// Creator mode is on: idle hint, app manager and system prompt follow it.
    creator: bool,
    /// Installed-app count, refreshed at mode/app lifecycle points so the idle
    /// hint can show it without a directory walk on every draw.
    apps_count: usize,
    /// Apps whose last run/test failed, shown as a warning suffix in the hint.
    broken_apps: usize,
    /// Set by the artifact viewer when Enter submits (`1`) rather than closing.
    artifact_action: u8,
    /// Display override for the next request when a prompt app was launched:
    /// the transcript shows this short label instead of the whole prompt body.
    prompt_label: Option<String>,
    /// Tool names called during the current turn (creator-mode nudge).
    turn_tools: Vec<String>,
    /// Bytes of a key sequence read while classifying it but not consumed by
    /// it; the next key read returns them first, so the legacy per-byte ESC
    /// handlers keep working next to the central PageUp/PageDown decoding.
    pending: std::cell::RefCell<std::collections::VecDeque<u8>>,
    /// Messages produced by background watchers: (model text, transcript label).
    auto_queue: Vec<(String, String)>,
    /// Label for the auto message edit() is about to return.
    auto_label: Option<String>,
    /// Last bare-Esc instant, for the double-Esc rewind gesture.
    last_esc: Option<std::time::Instant>,
    /// Recent tool calls (name, args, content, is_error, seconds), for CtrlO.
    tool_log: Vec<ToolLogEntry>,
    /// Raw watcher lines waiting for the next auto-turn window.
    watch_buffer: Vec<(String, String)>,
    /// Last time a watcher turn was triggered (rate limit).
    last_watch_trigger: Option<std::time::Instant>,
    /// Submitted input history (newest last), for ↑/↓ recall.
    history: Vec<String>,
    /// Position while browsing history; None = editing fresh input.
    hist_pos: Option<usize>,
    /// Draft preserved while browsing history.
    hist_draft: String,
    on_tick: Option<Box<dyn FnMut(&mut Scr)>>,
    /// Whale-girl session. Absent in unit tests that build a bare screen.
    pet: Option<pet_ui::PetHandle>,
}

/// One recorded tool call for the CtrlO viewer: the expanded arguments and the
/// settled output are kept together so a past call can be re-read in full.
#[derive(Clone)]
struct ToolLogEntry {
    name: String,
    args: String,
    content: String,
    is_error: bool,
    secs: f32,
}

/// Full-screen strict-mode confirmation panel.
struct ApprovalView {
    tool: String,
    args: String,
    reason: String,
}

/// Full-screen `ask_user` selection panel.
struct AskView {
    title: String,
    step: usize,
    total_steps: usize,
    prompt: String,
    allow_multiple: bool,
    options: Vec<String>,
    checked: Vec<bool>,
    sel: usize,
    typing_custom: bool,
    custom_buf: Vec<char>,
}

pub const K_NORM: u8 = 0;
pub const K_USER: u8 = 1;
pub const K_TOOL: u8 = 2;
pub const K_RES: u8 = 3;
pub const K_ERR: u8 = 4;
pub const K_DIM: u8 = 5;
pub const K_ART: u8 = 6;
pub const K_CODE: u8 = 7;
pub const K_HEAD: u8 = 8;
pub const K_STATE: u8 = 9;

/// 320 px / 6 px per cell. The TUI only ever runs on this panel, so the wrap
/// width is fixed here rather than trusted from the pty.
const PANEL_COLS: usize = 53;

/// Control bytes used for PageUp/PageDown: the device sends them as CSI 5~/6~
/// (fn+L / fn+M), so a single-byte code lets every screen match them without
/// its own sequence parser. Ctrl-K/Ctrl-L are unused by dst and map here too.
pub const KEY_PGUP: u8 = 0x0b;
pub const KEY_PGDN: u8 = 0x0c;

/// One decoded key. The panel's keyboard sends fn+F/X/Z/C as arrow keys and
/// fn+L/M as PageUp/PageDown; decoding them in one place keeps the transcript,
/// menus and the busy screen consistent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyEvent {
    Char(u8),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    /// Shift+Enter / Alt+Enter: submit without the palette.
    ShiftEnter,
    /// A lone Esc (cancel / dismiss).
    Esc,
}

/// What followed an already-consumed Esc byte.
enum EscChunk {
    /// Nothing: a bare Esc keypress.
    Bare,
    /// Esc + one plain byte (Alt+<key>).
    Alt(u8),
    /// A CSI/SS3 sequence: every byte that followed Esc, and the decoded key
    /// when the sequence is one dst knows.
    Seq(Vec<u8>, Option<KeyEvent>),
}

/// Decode a CSI/SS3 sequence body (`params` includes the final byte).
fn decode_seq(params: &[u8]) -> Option<KeyEvent> {
    let last = *params.last()?;
    let nums: Vec<u32> = params[..params.len().saturating_sub(1)]
        .split(|c| *c == b';')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            std::str::from_utf8(seg)
                .ok()
                .and_then(|t| t.parse::<u32>().ok())
                .unwrap_or(0)
        })
        .collect();
    let first = nums.first().copied().unwrap_or(0);
    match last {
        b'A' => Some(KeyEvent::Up),
        b'B' => Some(KeyEvent::Down),
        b'C' => Some(KeyEvent::Right),
        b'D' => Some(KeyEvent::Left),
        b'H' => Some(KeyEvent::Home),
        b'F' => Some(KeyEvent::End),
        b'~' => match first {
            1 | 7 => Some(KeyEvent::Home),
            4 | 8 => Some(KeyEvent::End),
            5 => Some(KeyEvent::PageUp),
            6 => Some(KeyEvent::PageDown),
            3 => Some(KeyEvent::Delete),
            _ => None,
        },
        b'u' => match first {
            13 => Some(KeyEvent::ShiftEnter),
            _ => None,
        },
        _ => None,
    }
}

/// Icon in front of the `state_report` progress line. A guillemet is in every
/// font the panel carries (Terminus + CJK fallback), unlike clock glyphs.
const STATE_ICON: &str = "\u{00bb}";


/// Retained transcript lines. The device has 512 MB and a turn can emit
/// hundreds of tool result lines; an unbounded `Vec` was the largest avoidable
/// allocation in a long session. Old lines are dropped from the front, which
/// keeps scrolling-into-the-past bounded but still useful.
const MAX_TRANSCRIPT_LINES: usize = 400;

/// Bounds for user-driven buffers that would otherwise be unbounded: a pasted
/// megabyte and a turn with thousands of tool calls must not grow them forever.
const MAX_INPUT: usize = 4096;
const MAX_QUEUE: usize = 16;

#[derive(Clone)]
struct MenuItem {
    label: String,
    value: String,
}

struct Menu {
    title: String,
    items: Vec<MenuItem>,
    sel: usize,
    hint: String,
    /// Expanded detail lines for the selected item (/resume previews a session).
    preview: Option<Vec<String>>,
}

impl Scr {
    fn new() -> Scr {
        let (r, c) = term_size();
        Scr {
            rows: r,
            cols: c,
            lines: Vec::new(),
            scroll: 0,
            input: Vec::new(),
            cursor: 0,
            note: None,
            quit: false,
            busy: false,
            voice_pending: false,
            pending_rec: None,
            voice_start_empty: None,
            speech_ok: true,
            think_lines: Vec::new(),
            think_active: false,
            art: art::Splash::new(),
            art_hold: 0,
            art_on: false,
            menu: None,
            palette_sel: 0,
            qr: None,
            qr_blit: false,
            ctx_window: 0,
            ctx_for: String::new(),
            mask: false,
            allow_empty: false,
            auto_send: true,
            utf8_buf: Vec::new(),
            artifact: None,
            splash_done: false,
            queue: Vec::new(),
            attachments: Vec::new(),
            live: None,
            state_line: None,
            update_offer: None,
            startup_notice: None,
            approval: None,
            ask: None,
            creator: false,
            apps_count: 0,
            broken_apps: 0,
            prompt_label: None,
            artifact_action: 0,
            turn_tools: Vec::new(),
            pending: std::cell::RefCell::new(std::collections::VecDeque::new()),
            auto_queue: Vec::new(),
            auto_label: None,
            last_esc: None,
            tool_log: Vec::new(),
            watch_buffer: Vec::new(),
            last_watch_trigger: None,
            history: Vec::new(),
            hist_pos: None,
            hist_draft: String::new(),
            on_tick: None,
            pet: None,
        }
    }

    /// Columns the transcript may use. 0 from the pet means "no dock".
    fn dock_columns(&self) -> usize {
        let full = self.cols.min(PANEL_COLS);
        let Some(pet) = &self.pet else { return 0 };
        let mut host = pet.borrow_mut();
        host.follow(self.art_on && !self.splash_done);
        host.dock_width(full)
    }

    fn transcript_width(&self) -> usize {
        let dock = self.dock_columns();
        if dock > 0 { dock } else { self.cols.min(PANEL_COLS) }
    }

    /// Finish a row. Dock rows are padded to the text width and not cleared
    /// to the edge, so the framebuffer pet in columns 43–52 is left alone.
    fn end_row(&self, s: &mut String, text: &str, dock_w: usize) {
        if dock_w > 0 {
            let fitted = fit(text, dock_w);
            let pad = dock_w.saturating_sub(dw(&fitted));
            s.push_str(&fitted);
            s.push_str(&" ".repeat(pad));
            s.push_str("\x1b[0m\r\n");
        } else {
            s.push_str(text);
            s.push_str("\x1b[0m\x1b[K\r\n");
        }
    }

    /// While the opening splash is up, the idle room replaces the character
    /// whale. Returns true when it drew a frame and the whale animation
    /// should stay put.
    fn pet_idle_frame(&mut self) -> bool {
        if !self.art_on || self.splash_done || self.lines.len() < 9 {
            return false;
        }
        let Some(pet) = self.pet.clone() else { return false };
        // Own the decision before touching `self`: the face borrows the host.
        let face = {
            let mut host = pet.borrow_mut();
            match host.idle_face() {
                pet_ui::IdleFace::Hidden => None,
                pet_ui::IdleFace::Pixel => Some(Vec::new()),
                pet_ui::IdleFace::Text(rows) => {
                    Some(rows.iter().take(9).map(|r| (*r).to_string()).collect())
                }
            }
        };
        let Some(rows) = face else { return false };
        if rows.is_empty() {
            for i in 0..9 {
                let text = if i == 8 { i18n::t("pet_room_hint").to_string() } else { String::new() };
                self.lines[i] = (text, K_DIM);
            }
        } else {
            for (i, row) in rows.into_iter().enumerate() {
                self.lines[i] = (row, K_ART);
            }
        }
        self.draw();
        true
    }

    fn cw(c: char) -> usize {
        let u = c as u32;
        // crude East-Asian wide detection, enough for CJK
        if (0x1100..=0x115F).contains(&u)
            || (0x2E80..=0xA4CF).contains(&u)
            || (0xAC00..=0xD7A3).contains(&u)
            || (0xF900..=0xFAFF).contains(&u)
            || (0xFE30..=0xFE6F).contains(&u)
            || (0xFF00..=0xFF60).contains(&u)
            || (0xFFE0..=0xFFE6).contains(&u)
            || (0x1F300..=0x1FAFF).contains(&u)
        {
            2
        } else {
            1
        }
    }

    /// Wrap to display cells, preferring spaces over mid-word breaks.
    fn wrap_into(out: &mut Vec<String>, text: &str, cols: usize) {
        for raw in text.split('\n') {
            if raw.is_empty() {
                out.push(String::new());
                continue;
            }
            let chars: Vec<char> = raw.chars().collect();
            let mut i = 0usize;
            while i < chars.len() {
                let mut w = 0usize;
                let mut j = i;
                let mut last_space: Option<usize> = None;
                while j < chars.len() {
                    let cw = Self::cw(chars[j]);
                    if w + cw > cols {
                        break;
                    }
                    w += cw;
                    if chars[j] == ' ' {
                        last_space = Some(j + 1);
                    }
                    j += 1;
                }
                if j >= chars.len() {
                    out.push(chars[i..].iter().collect());
                    break;
                }
                // Break at a space when one is reasonably near the edge.
                let mut end = match last_space {
                    Some(b) if b > i + cols / 2 => b,
                    _ => j.max(i + 1),
                };
                // A hard break can strand one or two characters on the next
                // line (a long saved-file path ended in a lone `t`); when the
                // overflow is that small, break at the preceding space.
                if end == j && chars.len() - j <= 3 {
                    if let Some(b) = last_space {
                        end = b;
                    }
                }
                out.push(chars[i..end].iter().collect::<String>().trim_end().to_string());
                i = end;
                while i < chars.len() && chars[i] == ' ' {
                    i += 1;
                }
            }
        }
    }

    /// Render model output: headings, bullets, code blocks, emphasis markers
    /// stripped. The screen is 12 rows, so structure matters more than markup.
    pub fn push_md(&mut self, text: &str) {
        let mut in_code = false;
        for raw in text.split('\n') {
            let trimmed = raw.trim_end();
            let t = trimmed.trim_start();
            if t.starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                self.push(&format!("  {trimmed}"), K_CODE);
                continue;
            }
            if t.is_empty() {
                self.push("", K_NORM);
                continue;
            }
            if let Some(h) = t
                .strip_prefix("### ")
                .or_else(|| t.strip_prefix("## "))
                .or_else(|| t.strip_prefix("# "))
            {
                self.push(&strip_inline(h), K_HEAD);
            } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                self.push(&format!("- {}", strip_inline(b)), K_NORM);
            } else if let Some((num, rest)) = numbered(t) {
                self.push(&format!("{num}. {}", strip_inline(rest)), K_NORM);
            } else {
                self.push(&strip_inline(t), K_NORM);
            }
        }
    }

    /// Re-read the installed-app count for the creator idle hint.
    pub fn refresh_apps_count(&mut self) {
        let apps = crate::apps::load();
        self.apps_count = apps.len();
        self.broken_apps = apps.iter().filter(|a| crate::apps::is_broken(a)).count();
    }

    /// Pin a progress line to the top row (`state_report`). A newer report
    /// replaces the previous one: the row shows what is happening *now*.
    pub fn set_state(&mut self, text: String) {
        self.state_line = Some(text);
        self.draw();
    }

    /// A progress line is live status, not history: when the next transcript
    /// line is about to appear, commit it to the log and free the pinned row.
    fn flush_state(&mut self) {
        let Some(text) = self.state_line.take() else { return };
        let mut wrapped = Vec::new();
        Self::wrap_into(
            &mut wrapped,
            &format!("{STATE_ICON} {text}"),
            self.transcript_width(),
        );
        for w in wrapped {
            self.lines.push((w, K_STATE));
        }
        if self.lines.len() > MAX_TRANSCRIPT_LINES {
            let drop = self.lines.len() - MAX_TRANSCRIPT_LINES;
            self.lines.drain(0..drop);
        }
    }

    /// Draw the first frame of the animated splash plus the hint line. Used at
    /// startup and whenever `/clear` resets the screen.
    pub fn seed_splash(&mut self, model: &str) {
        self.lines.clear();
        self.scroll = 0;
        self.splash_done = false;
        // The splash is a story: a seed always restarts it at step 0 of the
        // opening loop (never the sleeping idle loop), including after /clear.
        self.art = art::Splash::new();
        let (frame, ticks) = self.art.current();
        self.art_hold = ticks.saturating_sub(1);
        // A new screen must not inherit transient state: leaving the thinking
        // panel or the live line behind is what made `/clear` look half-done.
        self.think_lines.clear();
        self.think_active = false;
        self.attachments.clear();
        self.live = None;
        self.state_line = None;
        self.note = None;
        self.artifact = None;
        self.startup_notice = None;
        for (l, kind) in art_frame_lines(frame, self.cols) {
            self.lines.push((l, kind));
        }
        // One row is all the room under the 9-row art block; a long model name
        // is shortened so the key hints behind it stay readable.
        self.lines.push((splash_hint(model, self.cols), K_DIM));
        self.art_on = true;
        self.draw();
        self.pet_idle_frame();
    }

    /// Advance the splash animation while the user has not started working.
    ///
    /// One call = one idle key-poll tick (300 ms, see `read_key_timeout`). A
    /// frame only changes when its hold has expired, so calm/sleep frames cost
    /// one swap per 0.6-1.8 s while motion frames swap every 0.3 s; the
    /// top-of-loop `draw()` repaints the unchanged frame in between.
    fn anim_tick(&mut self) {
        if self.pet_idle_frame() {
            return;
        }
        if !self.art_on
            || !self.input.is_empty()
            || self.scroll != 0
            || self.lines.len() != art::ART_ROWS + 1
        {
            return;
        }
        if self.art_hold > 0 {
            self.art_hold -= 1;
            return;
        }
        let (frame, ticks) = self.art.next();
        self.art_hold = ticks.saturating_sub(1);
        for (i, (l, kind)) in art_frame_lines(frame, self.cols).into_iter().enumerate() {
            self.lines[i] = (l, kind);
        }
        self.draw();
    }

    pub fn push(&mut self, text: &str, kind: u8) {
        // The pinned `state_report` row is live status: tool cards and answer
        // text must not wipe it. It is replaced by the next report, and only
        // committed to the log when the turn ends (Ev::TurnEnd) or on /clear.
        let mut wrapped = Vec::new();
        let shown = tidy(text);
        Self::wrap_into(&mut wrapped, &shown, self.transcript_width());
        for w in wrapped {
            self.lines.push((w, kind));
        }
        // Keep the transcript bounded. Scrollback is measured from the end, so
        // dropping the oldest rows does not disturb the current view.
        if self.lines.len() > MAX_TRANSCRIPT_LINES {
            let drop = self.lines.len() - MAX_TRANSCRIPT_LINES;
            self.lines.drain(0..drop);
        }
        self.scroll = 0;
        self.draw();
    }

    /// Palette rows for the current input (empty when not composing a command).
    fn palette_items(&self) -> Vec<(&'static str, String, Option<&'static str>, &'static str)> {
        let text: String = self.input.iter().collect();
        cmd::palette(&text)
    }

    fn draw(&self) {
        if let Some(a) = &self.approval {
            self.draw_approval(a);
            return;
        }
        if let Some(q) = &self.ask {
            self.draw_ask(q);
            return;
        }
        if let Some(m) = &self.menu {
            self.draw_menu(m);
            return;
        }
        if let Some(lines) = &self.qr {
            if self.qr_blit {
                return;
            }
            let mut s = String::with_capacity(4096);
            s.push_str("\x1b[?25l\x1b[H");
            for i in 0..self.rows {
                if let Some(l) = lines.get(i) {
                    s.push_str(l);
                }
                s.push_str("\x1b[0m\x1b[K\r\n");
            }
            let mut out = std::io::stdout();
            let _ = out.write_all(s.as_bytes());
            let _ = out.flush();
            return;
        }
        if let Some(a) = &self.artifact {
            self.draw_artifact(a);
            return;
        }
        let mut s = String::with_capacity(4096);
        s.push_str("\x1b[?25l");
        s.push_str("\x1b[H");
        let dock_w = self.dock_columns();

        // ---- pinned progress row (`state_report`) ----
        // It owns the top row for as long as it is the newest activity, so the
        // user always sees the current stage without hunting in the log.
        if let Some(text) = &self.state_line {
            s.push_str("\x1b[1;35m");
            let width = if dock_w > 0 { dock_w } else { self.cols };
            self.end_row(&mut s, &fit(&format!("{STATE_ICON} {text}"), width), dock_w);
        }

        // Layout: [state(0/1)] + transcript + palette + live(1) + input(1).
        let pal = self.palette_items();
        let pal_rows = pal.len().min(4);
        let think_rows = if self.think_active {
            self.think_lines.len().min(4)
        } else {
            0
        };
        let state_rows = if self.state_line.is_some() { 1 } else { 0 };
        let body_rows = self
            .rows
            .saturating_sub(2)
            .max(1)
            .saturating_sub(pal_rows)
            .saturating_sub(think_rows)
            .saturating_sub(state_rows);

        // ---- transcript window ----
        let total = self.lines.len();
        let end = total.saturating_sub(self.scroll);
        let start = end.saturating_sub(body_rows);
        for i in 0..body_rows {
            let idx = start + i;
            if idx < end {
                let (txt, kind) = &self.lines[idx];
                match *kind {
                    K_USER => s.push_str("\x1b[1;36m"),
                    K_TOOL => s.push_str("\x1b[0;33m"),
                    K_RES => s.push_str("\x1b[90m"),
                    K_ERR => s.push_str("\x1b[0;31m"),
                    K_DIM => s.push_str("\x1b[90m"),
                    K_ART => s.push_str("\x1b[1;36m"),
                    K_CODE => s.push_str("\x1b[90m"),
                    K_STATE => s.push_str("\x1b[0;35m"),
                    K_HEAD => s.push_str("\x1b[1;37m"),
                    _ => s.push_str("\x1b[0m"),
                }
                if dock_w > 0 {
                    self.end_row(&mut s, txt, dock_w);
                    continue;
                }
                s.push_str(txt);
            }
            if dock_w > 0 {
                self.end_row(&mut s, "", dock_w);
            } else {
                s.push_str("\x1b[0m\x1b[K\r\n");
            }
        }

        // ---- live reasoning (folded once the answer begins) ----
        if think_rows > 0 {
            for line in self.think_lines.iter().skip(self.think_lines.len() - think_rows) {
                s.push_str("\x1b[35m");
                let width = if dock_w > 0 { dock_w } else { self.cols };
                self.end_row(&mut s, &fit(line, width), dock_w);
            }
        }

        // ---- command palette (only while composing a `/` command) ----
        let pal_start = if self.palette_sel >= pal_rows && pal_rows > 0 {
            self.palette_sel + 1 - pal_rows
        } else {
            0
        };
        for (vis, (name, desc, hint, cat)) in pal.iter().skip(pal_start).take(pal_rows).enumerate() {
            let i = pal_start + vis;
            let selected = i == self.palette_sel;
            let marker = if selected { "\u{25b8}" } else { " " };
            // Long argument hints would crowd out the category/description;
            // they remain visible in /help.
            let hint = hint
                .filter(|h| h.chars().count() <= 14)
                .map(|h| format!(" {h}"))
                .unwrap_or_default();
            let head = format!("{marker} /{name}{hint}");
            let pad = dw(&head);
            let room = self.cols.saturating_sub(pad + 3);
            let desc = format!("[{}] {desc}", i18n::t(cat));
            let d: String = desc.chars().take(room).collect();
            if selected {
                s.push_str("\x1b[1;36m");
            } else {
                s.push_str("\x1b[90m");
            }
            let width = if dock_w > 0 { dock_w } else { self.cols };
            self.end_row(&mut s, &fit(&format!("{head}   {d}"), width), dock_w);
        }

        // ---- live line: thinking / streaming tail / queue depth ----
        // One always-visible status line. Recording state used to be drawn
        // only in the header, which is hidden under fbterm/tmux - so pressing
        // the ASR key looked like it did nothing.
        let mut live_line = String::new();
        let mut hot = false;
        if let Some(n) = &self.note {
            live_line.push_str(&strip_ansi(n));
            hot = true;
        }
        if let Some((_, t)) = &self.live {
            if !live_line.is_empty() {
                live_line.push(' ');
            }
            live_line.push_str(t);
        }
        if live_line.is_empty() && !self.attachments.is_empty() {
            live_line = i18n::tf(
                "ui_image_ready",
                &[&self.attachments.len().to_string()],
            );
        }
        if live_line.is_empty() && !self.queue.is_empty() {
            live_line = format!(
                "{} \u{b7} CtrlQ",
                i18n::tf("queued", &[&self.queue.len().to_string()])
            );
        }
        // Scrolled back: say so, and how to get back.
        if self.busy && live_line.is_empty() {
            live_line = i18n::t("busy").to_string();
        }
        // While the palette is open, its footer doubles as a position/hint line.
        if live_line.is_empty() && !pal.is_empty() {
            live_line = format!(
                "{}/{}  \u{2191}\u{2193}  Enter  Esc",
                self.palette_sel.min(pal.len().saturating_sub(1)) + 1,
                pal.len()
            );
        }
        // Idle status line doubles as a one-time startup version banner, then
        // a key-hint area once dismissed.
        let mut cyan = false;
        if live_line.is_empty() && !self.busy && self.input.is_empty() && self.scroll == 0 {
            if let Some(v) = &self.startup_notice {
                live_line = v.clone();
                cyan = true;
            } else {
                live_line = if self.creator {
                    if self.broken_apps > 0 {
                        i18n::tf(
                            "creator_hint_broken",
                            &[&self.apps_count.to_string(), &self.broken_apps.to_string()],
                        )
                    } else {
                        i18n::tf("creator_hint", &[&self.apps_count.to_string()])
                    }
                } else {
                    i18n::t("idle_hint").to_string()
                };
            }
            hot = false;
        }
        if self.scroll > 0 {
            let hint = i18n::tf("scrolled", &[&self.scroll.to_string()]);
            if live_line.is_empty() {
                live_line = hint;
            } else {
                live_line = format!("{live_line} | {hint}");
            }
        }
        if !self.busy {
            if let Some(pet) = &self.pet {
                if let Some(b) = pet.borrow_mut().badge() {
                    live_line = if live_line.is_empty() {
                        b.to_string()
                    } else {
                        format!("{b} {live_line}")
                    };
                }
            }
        }
        if !live_line.is_empty() {
            s.push_str(if hot {
                "\x1b[31m"
            } else if cyan {
                "\x1b[36m"
            } else {
                "\x1b[90m"
            });
            s.push_str(&fit(&live_line, self.cols));
            s.push_str("\x1b[0m");
        }
        s.push_str("\x1b[K\r\n");

        // ---- input line: prompt + text, with the context ring pinned right ----
        let ring = if self.ctx_window > 0 {
            let pct = crate::usage::context_pct(self.ctx_window);
            let label = if pct > 0.0 && pct < 0.10 {
                format!("{:.1}%", pct * 100.0)
            } else {
                format!("{:.0}%", pct * 100.0)
            };
            format!("{} {}", crate::usage::ring(pct), label)
        } else {
            String::new()
        };
        // Status slot: context ring + goal badge + todo progress, greedily
        // dropping the ring first so the line never wraps on the 53-cell panel.
        let goal_badge = crate::goal::badge_cycle();
        let todo = crate::todo::progress().map(|t| format!("t{t}"));
        let mut parts: Vec<String> = Vec::new();
        if !ring.is_empty() { parts.push(ring.clone()); }
        if let Some(b) = &goal_badge { parts.push(b.clone()); }
        if let Some(t) = &todo { parts.push(t.clone()); }
        let cap = self.cols / 2;
        // Preference order: full, goal+todo, ring+goal, goal, ring+todo, ring.
        let mut choices: Vec<Vec<String>> = Vec::new();
        choices.push(parts.clone());
        if goal_badge.is_some() && todo.is_some() {
            choices.push(vec![goal_badge.clone().unwrap(), todo.clone().unwrap()]);
        }
        if goal_badge.is_some() {
            choices.push(vec![ring.clone(), goal_badge.clone().unwrap()]);
            choices.push(vec![goal_badge.clone().unwrap()]);
        }
        if todo.is_some() {
            choices.push(vec![ring.clone(), todo.clone().unwrap()]);
        }
        choices.push(vec![ring.clone()]);
        let mut right = choices
            .iter()
            .map(|c| c.join(" "))
            .find(|c| dw(c) <= cap)
            .unwrap_or_else(|| fit(&parts.join(" "), cap));
        if dw(&right) > cap {
            right = fit(&right, cap);
        }
        let ring = right;
        let ring_w = dw(&ring);
        s.push_str("\x1b[1;33m> \x1b[0m");
        let avail = self.cols.saturating_sub(2 + if ring_w > 0 { ring_w + 1 } else { 0 });
        let cur = self.cursor.min(self.input.len());
        // One-line input window around the cursor: fill backwards for context,
        // then forwards, and spend one cell per "…" marker when either side is
        // hidden. Without the forward pass, Ctrl-A on a long prompt printed the
        // whole string and wrapped over the status row.
        let mut lead = false;
        let mut trail = false;
        let mut start_i = cur;
        let mut end_i = cur;
        for _ in 0..3 {
            let markers = usize::from(lead) + usize::from(trail);
            let budget = avail.saturating_sub(markers);
            let mut w = 0usize;
            start_i = cur;
            while start_i > 0 {
                let cw = Self::cw(self.input[start_i - 1]);
                if w + cw > budget {
                    break;
                }
                w += cw;
                start_i -= 1;
            }
            end_i = cur;
            while end_i < self.input.len() {
                let cw = Self::cw(self.input[end_i]);
                if w + cw > budget {
                    break;
                }
                w += cw;
                end_i += 1;
            }
            let new_lead = start_i > 0;
            let new_trail = end_i < self.input.len();
            if (new_lead, new_trail) == (lead, trail) {
                break;
            }
            lead = new_lead;
            trail = new_trail;
        }
        if lead {
            s.push_str("\u{2026}");
        }
        let body: String = self.input[start_i..end_i].iter().collect();
        if self.mask {
            s.push_str(&"*".repeat(body.chars().count()));
        } else {
            s.push_str(&body);
        }
        if trail {
            s.push_str("\u{2026}");
        }
        s.push_str("\x1b[K");

        // Cursor column = width of everything BEFORE the cursor in the window.
        // Right-aligned ring on the same row as the input.
        if ring_w > 0 {
            let rcol = self.cols.saturating_sub(ring_w) + 1;
            s.push_str(&format!("\x1b[{};{}H\x1b[90m{}\x1b[0m", self.rows, rcol, ring));
        }
        let col = 2 + usize::from(lead) + self.input[start_i..cur]
            .iter()
            .map(|c| Self::cw(*c))
            .sum::<usize>();
        let col = col.min(self.cols.saturating_sub(1));
        s.push_str(&format!("\x1b[{};{}H\x1b[?25h", self.rows, col + 1));

        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
        if let Some(pet) = &self.pet {
            pet.borrow_mut().present();
        }
    }

    fn draw_approval(&self, a: &ApprovalView) {
        let mut s = String::with_capacity(2048);
        s.push_str("\x1b[?25l\x1b[2J\x1b[H");
        s.push_str("\x1b[1;31m");
        s.push_str(&fit(&format!(" {}", i18n::t("approval_title")), self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n\r\n");
        s.push_str("\x1b[1;37m");
        s.push_str(&fit(&format!(" {} {}", i18n::t("approval_tool"), a.tool), self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n\r\n");
        let mut shown = 0usize;
        for raw in a.args.lines() {
            // Colour removed/added lines so an edit is readable at a glance;
            // the framebuffer terminal has 8 colours, red/green are safe.
            let colour = if raw.trim_start().starts_with("- ") {
                "\x1b[0;31m"
            } else if raw.trim_start().starts_with("+ ") {
                "\x1b[0;32m"
            } else {
                "\x1b[0;37m"
            };
            for line in wrap(raw) {
                if shown >= 6 {
                    break;
                }
                s.push_str(colour);
                s.push_str(&fit(&format!(" {line}"), self.cols));
                s.push_str("\x1b[0m\x1b[K\r\n");
                shown += 1;
            }
            if shown >= 6 {
                break;
            }
        }
        s.push_str("\x1b[90m");
        s.push_str(&fit(
            &format!(" {} {}", i18n::t("approval_reason"), a.reason),
            self.cols,
        ));
        s.push_str("\x1b[0m\x1b[K\r\n\r\n");
        s.push_str("\x1b[1;33m");
        s.push_str(&fit(&format!(" {}", i18n::t("approval_keys")), self.cols));
        s.push_str("\x1b[0m\x1b[K");
        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    fn draw_ask(&self, v: &AskView) {
        let mut s = String::with_capacity(2048);
        s.push_str("\x1b[?25l\x1b[2J\x1b[H");
        // Header row: title on left, step or scroll counter on right.
        let counter = if v.total_steps > 1 {
            format!("{}/{} ", v.step + 1, v.total_steps)
        } else {
            String::new()
        };
        let counter_w = dw(&counter);
        let title_room = self.cols.saturating_sub(counter_w);
        let mut head = fit(&format!(" {} ", v.title), title_room);
        head.push_str(&" ".repeat(title_room.saturating_sub(dw(&head))));
        head.push_str(&counter);
        s.push_str("\x1b[1;36m");
        s.push_str(&fit(&head, self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n");

        // Prompt text: wrapped up to 2 rows.
        let mut prompt_lines: Vec<String> = Vec::new();
        Scr::wrap_into(&mut prompt_lines, &v.prompt, self.cols.saturating_sub(1).max(10));
        prompt_lines.truncate(2);
        for pl in &prompt_lines {
            s.push_str("\x1b[1;37m");
            s.push_str(&fit(&format!(" {pl}"), self.cols));
            s.push_str("\x1b[0m\x1b[K\r\n");
        }

        let body_rows = self
            .rows
            .saturating_sub(2 + prompt_lines.len())
            .max(1);

        if v.typing_custom {
            s.push_str("\x1b[K\r\n");
            let typed: String = v.custom_buf.iter().collect();
            let shown = fit(&format!(" > {typed}"), self.cols);
            s.push_str("\x1b[1;33m");
            s.push_str(&shown);
            s.push_str("\x1b[0m\x1b[K\r\n");
            for _ in 2..body_rows {
                s.push_str("\x1b[K\r\n");
            }
            s.push_str("\x1b[90m");
            s.push_str(&fit_ellipsis(i18n::t("ask_custom_hint"), self.cols));
            s.push_str("\x1b[0m\x1b[K");
            // Place cursor at end of custom input.
            let cur_row = 1 + prompt_lines.len() + 2;
            let cur_col = (dw(&shown) + 1).min(self.cols);
            s.push_str(&format!("\x1b[{cur_row};{cur_col}H\x1b[?25h"));
        } else {
            let total_rows = v.options.len() + 1; // +1 for "Other..."
            let start = if v.sel >= body_rows {
                v.sel + 1 - body_rows
            } else {
                0
            };
            for i in 0..body_rows {
                let idx = start + i;
                if idx < total_rows {
                    let selected = idx == v.sel;
                    let mark = if selected { "\u{25b8}" } else { " " };
                    let text = if idx < v.options.len() {
                        if v.allow_multiple {
                            let box_s = if v.checked.get(idx).copied().unwrap_or(false) {
                                "[x]"
                            } else {
                                "[ ]"
                            };
                            format!("{mark} {box_s} {}", v.options[idx])
                        } else {
                            format!("{mark} {}. {}", idx + 1, v.options[idx])
                        }
                    } else {
                        format!("{mark} {}", i18n::t("ask_other"))
                    };
                    let mut line = fit_ellipsis(&text, self.cols);
                    let pad = self.cols.saturating_sub(dw(&line));
                    line.push_str(&" ".repeat(pad));
                    if selected {
                        s.push_str("\x1b[7m");
                    } else if idx == v.options.len() {
                        s.push_str("\x1b[0;36m");
                    } else {
                        s.push_str("\x1b[0;37m");
                    }
                    s.push_str(&fit(&line, self.cols));
                    s.push_str("\x1b[0m\x1b[K\r\n");
                } else {
                    s.push_str("\x1b[K\r\n");
                }
            }
            let hint_key = if v.allow_multiple {
                "ask_hint_multi"
            } else {
                "ask_hint_single"
            };
            s.push_str("\x1b[90m");
            s.push_str(&fit_ellipsis(i18n::t(hint_key), self.cols));
            s.push_str("\x1b[0m\x1b[K");
        }
        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    fn draw_menu(&self, m: &Menu) {
        let mut s = String::with_capacity(2048);
        // Full-screen panel: clear so no transcript shows through.
        s.push_str("\x1b[?25l\x1b[2J\x1b[H");
        let preview_rows = m.preview.as_ref().map(|p| p.len()).unwrap_or(0);
        let rows = self
            .rows
            .saturating_sub(2)
            .max(1)
            .saturating_sub(preview_rows);
        // Long lists show a scroll position on the right of the title.
        let counter = if m.items.len() > rows {
            format!("{}/{} ", m.sel + 1, m.items.len())
        } else {
            String::new()
        };
        let counter_w = dw(&counter);
        s.push_str("\x1b[1;36m");
        let title = format!(" {} ", m.title);
        let title_room = self.cols.saturating_sub(counter_w);
        let mut head = fit(&title, title_room);
        head.push_str(&" ".repeat(title_room.saturating_sub(dw(&head))));
        head.push_str(&counter);
        s.push_str(&fit(&head, self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n");
        // Window the list around the selection (menuconfig behaviour).
        let start = if m.sel >= rows { m.sel + 1 - rows } else { 0 };
        // Labels stay left-aligned; values sit on the right edge so every row
        // fills the panel width (selection highlight spans the whole line).
        for i in 0..rows {
            let idx = start + i;
            if let Some(item) = m.items.get(idx) {
                let selected = idx == m.sel;
                let mark = if selected { "\u{25b8}" } else { " " };
                // Reserve at least 12 cells for the label; long right-hand
                // text is truncated instead of squeezing the label.
                let value = fit_ellipsis(&item.value, self.cols / 2 - 2);
                let value_w = dw(&value);
                // Reserve the selection marker, its space, the label and at
                // least two cells of breathing room before the value.
                let label_room = self
                    .cols
                    .saturating_sub(if value_w > 0 { value_w + 4 } else { 2 })
                    .max(12);
                let label = format!("{mark} {}", fit_ellipsis(&item.label, label_room));
                let gap = self.cols.saturating_sub(dw(&label) + value_w);
                let line = format!("{label}{}{}", " ".repeat(gap), value);
                if selected {
                    s.push_str("\x1b[7m");
                } else {
                    s.push_str("\x1b[0;37m");
                }
                s.push_str(&fit(&line, self.cols));
                s.push_str("\x1b[0m\x1b[K\r\n");
            } else {
                s.push_str("\x1b[K\r\n");
            }
        }
        if let Some(preview) = &m.preview {
            for line in preview {
                s.push_str("\x1b[0;36m");
                s.push_str(&fit_ellipsis(line, self.cols));
                s.push_str("\x1b[0m\x1b[K\r\n");
            }
        }
        s.push_str("\x1b[90m");
        s.push_str(&fit_ellipsis(&m.hint, self.cols));
        s.push_str("\x1b[0m\x1b[K");
        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    fn artifact_body_rows(&self) -> usize {
        self.rows.saturating_sub(2).max(1)
    }

    fn draw_artifact(&self, a: &Artifact) {
        let mut s = String::with_capacity(4096);
        s.push_str("\x1b[?25l\x1b[H");
        let total = a.total_pages();
        let page = a.current_page();
        let right = if total > 1 {
            format!("{}/{}", a.page + 1, total)
        } else if let Some(sub) = &a.subtitle {
            sub.clone()
        } else {
            String::new()
        };
        let heading = match &page.title {
            Some(pt) if !pt.is_empty() => format!("{} \u{b7} {pt}", a.title),
            _ => a.title.clone(),
        };
        let right_w = dw(&right);
        let left_room = if right_w > 0 {
            self.cols.saturating_sub(right_w + 3).max(8)
        } else {
            self.cols.saturating_sub(1).max(8)
        };
        let left = format!(" {}", fit_ellipsis(&heading, left_room));
        let gap = self.cols.saturating_sub(dw(&left) + right_w);
        let header = if right_w > 0 {
            format!("{left}{}{right}", " ".repeat(gap))
        } else {
            left
        };
        s.push_str(a.theme.header_sgr());
        s.push_str(&fit(&header, self.cols));
        s.push_str("\x1b[0m\x1b[K\r\n");

        // Fixed-size page canvas: every page occupies exactly `body_rows` rows.
        let body_rows = self.artifact_body_rows();
        for i in 0..body_rows {
            if let Some(l) = page.lines.get(i) {
                s.push_str(&crate::artifact::clip_ansi(l, self.cols));
            }
            s.push_str("\x1b[0m\x1b[K\r\n");
        }

        // With a save notice, the notice is what the user needs; the key
        // hints come back on the next page flip or after dismissal.
        let page_badge = format!("{}/{}", a.page + 1, total);
        let footer = match (&a.notice, a.submit, total > 1) {
            (Some(n), _, _) => format!(" {} | {}", i18n::t("artifact_back"), n),
            (None, true, true) => format!(
                " {} | {} | {} {} | s save",
                i18n::t("artifact_send"),
                i18n::t("artifact_back"),
                i18n::t("artifact_page"),
                page_badge
            ),
            (None, true, false) => format!(
                " {} | {} | s save",
                i18n::t("artifact_send"),
                i18n::t("artifact_back")
            ),
            (None, false, true) => format!(
                " {} | {} {} | s save",
                i18n::t("artifact_back"),
                i18n::t("artifact_page"),
                page_badge
            ),
            (None, false, false) => match &a.footer {
                Some(f) if !f.is_empty() => format!(" {} | s save | {f}", i18n::t("artifact_back")),
                _ => format!(" {} | s save", i18n::t("artifact_back")),
            },
        };
        s.push_str("\x1b[90m");
        s.push_str(&fit_dots(&footer, self.cols));
        s.push_str("\x1b[0m\x1b[K");
        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    /// Assemble a multibyte UTF-8 character from raw input bytes. Without this
    /// every non-ASCII byte was dropped, so Chinese could not be typed or
    /// pasted into the prompt at all.
    fn utf8_push(&mut self, b: u8) {
        if self.utf8_buf.is_empty() && b < 0x80 {
            self.input.insert(self.cursor, b as char);
            self.cursor += 1;
            return;
        }
        self.utf8_buf.push(b);
        let need = match self.utf8_buf[0] {
            0xf0..=0xf4 => 4,
            0xe0..=0xef => 3,
            0xc0..=0xdf => 2,
            _ => 1,
        };
        if self.utf8_buf.len() >= need {
            if let Ok(s) = std::str::from_utf8(&self.utf8_buf) {
                if let Some(ch) = s.chars().next() {
                    self.input.insert(self.cursor, ch);
                    self.cursor += 1;
                }
            }
            self.utf8_buf.clear();
        }
    }

    /// Dispatch one decoded key event to the active fixed-page artifact viewer.
    fn handle_artifact_event(&mut self, ev: KeyEvent) -> bool {
        match ev {
            KeyEvent::Char(b's') | KeyEvent::Char(b'S') => {
                if let Some(a) = self.artifact.as_mut() {
                    // Keep the file name short enough that the saved-path
                    // notice fits next to the footer's Esc hint on the
                    // 53-cell panel (long artifact titles used to be cut
                    // mid-word).
                    let name: String = a
                        .title
                        .chars()
                        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
                        .take(20)
                        .collect();
                    let name = match name.rfind('-') {
                        Some(pos) if pos >= 8 => name[..pos].to_string(),
                        _ => name,
                    };
                    let name = name.trim_end_matches('-').to_string();
                    let name = if name.is_empty() { "artifact".to_string() } else { name };
                    let path = crate::config::tmp_dir().join(format!("{name}.txt"));
                    match std::fs::write(&path, &a.save_text) {
                        Ok(()) => {
                            let home = std::env::var("HOME").unwrap_or_default();
                            let mut shown = path.display().to_string();
                            if !home.is_empty() && shown.starts_with(&home) {
                                shown = format!("~{}", &shown[home.len()..]);
                            }
                            a.notice = Some(i18n::tf("ui_saved", &[&shown]));
                        }
                        Err(e) => {
                            a.notice = Some(i18n::tf("ui_save_failed", &[&e.to_string()]))
                        }
                    }
                    self.draw();
                }
                true
            }
            KeyEvent::Char(0x0d) | KeyEvent::Char(0x0a) => {
                if let Some(a) = &self.artifact {
                    if a.submit {
                        self.artifact_action = 1;
                    }
                }
                self.artifact = None;
                self.draw();
                true
            }
            KeyEvent::Esc | KeyEvent::Char(b'q') | KeyEvent::Char(b'Q') | KeyEvent::Char(0x03) => {
                self.artifact = None;
                self.draw();
                true
            }
            KeyEvent::Right
            | KeyEvent::Down
            | KeyEvent::PageDown
            | KeyEvent::Char(b' ')
            | KeyEvent::Char(b'n')
            | KeyEvent::Char(b'N')
            | KeyEvent::Char(b'l')
            | KeyEvent::Char(b'L') => {
                if let Some(a) = self.artifact.as_mut() {
                    if a.next_page() {
                        self.draw();
                    }
                }
                true
            }
            KeyEvent::Left
            | KeyEvent::Up
            | KeyEvent::PageUp
            | KeyEvent::Char(0x7f)
            | KeyEvent::Char(0x08)
            | KeyEvent::Char(b'p')
            | KeyEvent::Char(b'P')
            | KeyEvent::Char(b'h')
            | KeyEvent::Char(b'H') => {
                if let Some(a) = self.artifact.as_mut() {
                    if a.prev_page() {
                        self.draw();
                    }
                }
                true
            }
            KeyEvent::Home | KeyEvent::Char(b'g') => {
                if let Some(a) = self.artifact.as_mut() {
                    if a.first_page() {
                        self.draw();
                    }
                }
                true
            }
            KeyEvent::End | KeyEvent::Char(b'G') => {
                if let Some(a) = self.artifact.as_mut() {
                    if a.last_page() {
                        self.draw();
                    }
                }
                true
            }
            KeyEvent::Char(d @ b'1'..=b'9') => {
                if let Some(a) = self.artifact.as_mut() {
                    if a.total_pages() > 1 && a.goto_page((d - b'1') as usize) {
                        self.draw();
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Blocking-with-timeout keystroke while a panel is up in idle/modal loops.
    fn artifact_keystroke(&mut self) -> bool {
        if self.artifact.is_none() {
            return false;
        }
        match self.read_key_event() {
            Some(ev) => self.handle_artifact_event(ev),
            None => false,
        }
    }

    /// Non-blocking drain of artifact keystrokes during a running turn's `pump()`,
    /// so keeping an artifact or `/btw` panel open never stalls the background turn.
    fn artifact_keystroke_now(&mut self) -> bool {
        let mut handled = false;
        while self.artifact.is_some() {
            match self.read_key_event_now() {
                Some(ev) => {
                    handled |= self.handle_artifact_event(ev);
                }
                None => break,
            }
        }
        handled
    }

    fn pop_pending(&self) -> Option<u8> {
        self.pending.borrow_mut().pop_front()
    }

    /// Put bytes back at the head of the input queue (in read order).
    fn push_back(&self, bytes: &[u8]) {
        let mut q = self.pending.borrow_mut();
        for b in bytes.iter().rev() {
            q.push_front(*b);
        }
    }

    /// Non-blocking key read. `poll_mode(true)` sets VMIN=0/VTIME=0 so this
    /// returns None immediately instead of waiting for a key.
    fn read_key_now(&self) -> Option<u8> {
        if let Some(b) = self.pop_pending() {
            return Some(b);
        }
        let mut b = [0u8; 1];
        let n = std::io::stdin().read(&mut b).unwrap_or(0);
        if n == 1 { Some(b[0]) } else { None }
    }

    #[allow(dead_code)] // kept for future blocking reads; the TUI is event based
    fn read_key(&self) -> u8 {
        let mut b = [0u8; 1];
        let mut si = std::io::stdin();
        loop {
            if let Some(b) = self.pop_pending() {
                return b;
            }
            match si.read(&mut b) {
                Ok(1) => return b[0],
                Ok(_) => continue,
                Err(_) => return 4,
            }
        }
    }

    /// A byte that may follow Esc (arrow/function-key sequences). Blocks for a
    /// very short window: sequence bytes are written together, so a real Esc
    /// tap pays only a few milliseconds and a sequence is never mistaken for
    /// a bare Esc (that race swallowed arrow/PageUp presses on the device).
    fn read_key_after_esc(&self) -> Option<u8> {
        if let Some(b) = self.pop_pending() {
            return Some(b);
        }
        poll_mode(true);
        let t0 = std::time::Instant::now();
        loop {
            let mut b = [0u8; 1];
            let n = std::io::stdin().read(&mut b).unwrap_or(0);
            if n == 1 {
                return Some(b[0]);
            }
            if t0.elapsed().as_millis() >= 60 {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Read one raw byte, waiting up to `tenths` tenths of a second.
    fn read_byte_timeout(&self, tenths: u8) -> Option<u8> {
        if let Some(b) = self.pop_pending() {
            return Some(b);
        }
        let t = tenths.to_string();
        stty_tty(&["min", "0", "time", t.as_str()]);
        let mut b = [0u8; 1];
        let n = std::io::stdin().read(&mut b).unwrap_or(0);
        stty_tty(&["min", "1", "time", "0"]);
        if n == 1 { Some(b[0]) } else { None }
    }

    /// Wait up to `ms` for one more input byte without blocking the caller for
    /// long (used while finishing a key sequence).
    fn wait_byte(&self, ms: u64) -> Option<u8> {
        if let Some(b) = self.pop_pending() {
            return Some(b);
        }
        poll_mode(true);
        let t0 = std::time::Instant::now();
        loop {
            let mut b = [0u8; 1];
            let n = std::io::stdin().read(&mut b).unwrap_or(0);
            if n == 1 {
                return Some(b[0]);
            }
            if t0.elapsed().as_millis() >= ms as u128 {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Classify what follows an Esc byte that was already read. Consumes the
    /// bytes in the queue order they are returned: the caller decides which of
    /// them to keep.
    fn read_esc_chunk(&self, wait_ms: u64) -> EscChunk {
        let Some(b1) = self.wait_byte(wait_ms) else {
            return EscChunk::Bare;
        };
        if b1 != b'[' && b1 != b'O' {
            return EscChunk::Alt(b1);
        }
        let mut all = vec![b1];
        let mut params: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.wait_byte(wait_ms) else {
                return EscChunk::Seq(all, None);
            };
            all.push(c);
            params.push(c);
            if c.is_ascii_alphabetic() || c == b'~' || c == b'u' || params.len() >= 8 {
                break;
            }
        }
        let key = decode_seq(&params);
        EscChunk::Seq(all, key)
    }

    /// Turn an already-read Esc into a key event.
    fn esc_to_event(&self) -> KeyEvent {
        match self.read_esc_chunk(120) {
            EscChunk::Bare => KeyEvent::Esc,
            EscChunk::Alt(x) => {
                if x == b'\r' || x == b'\n' {
                    KeyEvent::ShiftEnter
                } else {
                    KeyEvent::Char(x)
                }
            }
            EscChunk::Seq(_, Some(k)) => k,
            EscChunk::Seq(_, None) => KeyEvent::Esc,
        }
    }

    /// Blocking key read that decodes the panel's key sequences (up to ~300 ms).
    fn read_key_event(&self) -> Option<KeyEvent> {
        let b = self.read_byte_timeout(3)?;
        if b != 0x1b {
            return Some(KeyEvent::Char(b));
        }
        Some(self.esc_to_event())
    }

    /// Non-blocking variant for the busy screen (`pump` must not stall the
    /// request loop).
    fn read_key_event_now(&self) -> Option<KeyEvent> {
        match self.read_key_now() {
            Some(b) if b == 0x1b => Some(self.esc_to_event()),
            Some(b) => Some(KeyEvent::Char(b)),
            None => None,
        }
    }

    /// Blocking byte reader used by older screens. PageUp/PageDown come back
    /// as `KEY_PGUP`/`KEY_PGDN`; every other sequence is left in the queue as
    /// its original bytes so the per-screen ESC handling still sees them.
    fn read_key_timeout(&self) -> Option<u8> {
        let b = self.read_byte_timeout(3)?;
        if b != 0x1b {
            return Some(b);
        }
        match self.read_esc_chunk(120) {
            // Lists understand arrows already, so a page key is replayed as a
            // screenful of Up/Down: paging then works in every menu without a
            // second key contract. The toast screens (pump/edit) use
            // read_key_event and get the real PageUp/PageDown instead.
            EscChunk::Seq(_, Some(KeyEvent::PageUp)) => {
                self.queue_arrows(b'A');
                // The key's own ESC byte is the first queued one: consume it
                // so the legacy ESC handler reads the '[' next, exactly like a
                // real arrow sequence.
                self.pop_pending();
                Some(0x1b)
            }
            EscChunk::Seq(_, Some(KeyEvent::PageDown)) => {
                self.queue_arrows(b'B');
                self.pop_pending();
                Some(0x1b)
            }
            EscChunk::Seq(bytes, _) => {
                self.push_back(&bytes);
                Some(0x1b)
            }
            EscChunk::Alt(x) => {
                self.push_back(&[x]);
                Some(0x1b)
            }
            EscChunk::Bare => Some(0x1b),
        }
    }

    /// Replay one page key as a screenful of arrow keys for list screens.
    fn queue_arrows(&self, final_byte: u8) {
        let n = self.rows.saturating_sub(4).max(3);
        let seq = [0x1b, b'[', final_byte];
        for _ in 0..n {
            self.push_back(&seq);
        }
    }

    /// Visible transcript rows, mirroring `draw()`'s layout.
    fn body_rows(&self) -> usize {
        let pal_rows = self.palette_items().len().min(4);
        let think_rows = if self.think_active {
            self.think_lines.len().min(4)
        } else {
            0
        };
        let state_rows = if self.state_line.is_some() { 1 } else { 0 };
        self.rows
            .saturating_sub(2)
            .max(1)
            .saturating_sub(pal_rows)
            .saturating_sub(think_rows)
            .saturating_sub(state_rows)
    }

    /// Scroll the transcript window. `n == usize::MAX` jumps to the end (or to
    /// the very top when scrolling back). The window is clamped so the top of
    /// the buffer stops at the top row instead of scrolling into blankness.
    fn scroll_view(&mut self, back: bool, n: usize) {
        let rows = self.body_rows().max(1);
        let max = self.lines.len().saturating_sub(rows);
        if back {
            self.scroll = if n == usize::MAX {
                max
            } else {
                (self.scroll + n).min(max)
            };
        } else {
            self.scroll = if n == usize::MAX {
                0
            } else {
                self.scroll.saturating_sub(n)
            };
        }
    }

    /// Wait for the key to be genuinely held. Key-repeat starts after 400 ms,
    /// so three TAB bytes (~490 ms) mean "hold"; one byte then silence is a tap.
    fn arm_tab(&mut self) -> bool {
        let t0 = std::time::Instant::now();
        let mut n = 0;
        loop {
            match self.read_key_timeout() {
                Some(0x09) => {
                    n += 1;
                    if n >= 3 {
                        return true;
                    }
                }
                Some(0x03) | Some(0x1b) => return false,
                Some(_) => {}
                None => return false,
            }
            if t0.elapsed().as_millis() > 900 && n < 3 {
                return false;
            }
        }
    }

    /// Long-press Tab => record, release => transcribe with DeepSeek ASR.
    fn vlog(&self, msg: &str) {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::log_dir().join("voice.log"))
        {
            let _ = writeln!(f, "{msg}");
        }
    }

    fn voice_input(&mut self) -> bool {
        self.voice_capture(None)
    }

    /// Hold-to-talk capture. `pre` is a recorder the busy screen already opened
    /// when Tab interrupted the turn: the mic is live before the turn unwinds,
    /// so the first words are not lost, and the tap debounce is skipped because
    /// the keypress was deliberate (the user asked to talk, not to type).
    fn voice_capture(&mut self, pre: Option<crate::voice::Recorder>) -> bool {
        // The mode is decided when the talk key went down: empty prompt means
        // "talk and send" (hands-free), a non-empty prompt means dictation.
        let start_empty = self.voice_start_empty.take().unwrap_or_else(|| self.input.is_empty());
        if !self.speech_ok {
            // A pre-opened recorder must not be dropped silently: dropping it
            // stops the capture (Drop impl), which is exactly the intent.
            self.push(i18n::t("voice_official_only"), K_ERR);
            return false;
        }
        let resume = pre.is_some();
        // Open the mic BEFORE the hold is confirmed: the codec takes a moment
        // to settle and starting late clipped the first syllable of speech.
        // The pre-roll is discarded by the arming check below.
        let rec = match pre {
            Some(r) => r,
            None => match crate::voice::start_recording() {
                Ok(r) => r,
                Err(e) => {
                    self.push(&i18n::tf("ui_mic_failed", &[&e.to_string()]), K_ERR);
                    return false;
                }
            },
        };
        // Debounce: a tap stays a normal Tab. Otherwise ~0.5 s of room noise
        // reaches ASR and comes back as an invented sentence.
        if !resume && !self.arm_tab() {
            rec.cancel();
            self.vlog("tap (not armed) - no recording");
            return false;
        }
        self.vlog(if resume { "armed (interrupt)" } else { "armed" });
        let t0 = std::time::Instant::now();
        self.note = Some("\x1b[31m\u{25cf} REC\x1b[90m 0.0s".into());
        self.draw();
        let mut cancelled = false;
        // A held key auto-repeats only after ~400 ms; an interrupt take must
        // not mistake that quiet stretch for a release (it would stop
        // recording before the user finished). The debounce path already
        // proved the key is down, so it may stop on the first quiet window.
        let mut quiet_ms = 0u32;
        loop {
            match self.read_key_timeout() {
                Some(0x09) => {
                    quiet_ms = 0;
                    self.note = Some(format!(
                        "\x1b[31m\u{25cf} REC\x1b[90m {:.1}s",
                        t0.elapsed().as_secs_f32()
                    ));
                    self.draw();
                    if t0.elapsed().as_secs_f32() > 30.0 {
                        break; // hard cap
                    }
                }
                Some(0x03) | Some(0x1b) => {
                    cancelled = true;
                    break;
                }
                Some(_) => quiet_ms = 0,
                None => {
                    quiet_ms += 300;
                    let limit = if resume { 900 } else { 300 };
                    if quiet_ms >= limit {
                        break; // key released
                    }
                }
            }
        }
        if cancelled {
            rec.cancel();
            self.note = None;
            self.draw();
            return false;
        }
        self.note = Some("\x1b[90m\u{2026} transcribing".into());
        self.draw();
        let level = rec.level();
        let secs = t0.elapsed().as_secs_f32();
        let text = match rec.finish() {
            Ok(t) => t,
            Err(e) => {
                self.vlog(&format!("take {secs:.1}s level={level:.4} error={e}"));
                self.note = None;
                self.draw();
                self.push(&i18n::tf("ui_speech_failed", &[&e.to_string()]), K_ERR);
                return false;
            }
        };
        self.vlog(&format!(
            "take {secs:.1}s level={level:.4} chars={} text={:?}",
            text.chars().count(),
            text.chars().take(40).collect::<String>()
        ));
        self.note = None;
        // Very short take or a one-character result is noise, not speech.
        if t0.elapsed().as_secs_f32() < 0.6 || text.chars().count() < 2 {
            self.draw();
            return false;
        }
        let send = start_empty && self.auto_send;
        self.vlog(if send { "mode=send" } else { "mode=dictation" });
        for ch in tidy(&text).chars() {
            self.input.insert(self.cursor, ch);
            self.cursor += 1;
        }
        self.draw();
        send
    }

    /// Interactive manager for background jobs and watchers: Up/Down select,
    /// Enter shows the output tail, `x` kills the task, Esc closes.
    fn jobs_panel(&mut self) {
        let mut sel = 0usize;
        let mut preview: Option<Vec<String>> = None;
        let mut filter = 0u8; // 0 all, 1 running, 2 watchers
        loop {
            let mut views = crate::bg::views();
            if views.is_empty() {
                self.push(i18n::t("jobs_none"), K_DIM);
                return;
            }
            views.sort_by(|a, b| {
                let ar = a.status.starts_with("running");
                let br = b.status.starts_with("running");
                br.cmp(&ar)
            });
            views.retain(|v| match filter {
                1 => v.status.starts_with("running"),
                2 => v.watcher,
                _ => true,
            });
            if sel >= views.len() {
                sel = views.len().saturating_sub(1);
            }
            let items: Vec<MenuItem> = views
                .iter()
                .map(|v| MenuItem {
                    label: format!(
                        "{} {}  {}",
                        if v.watcher { "\u{2248}" } else { "\u{25cf}" },
                        v.id,
                        first_line_short(&v.command, 22)
                    ),
                    value: format!("  {} \u{b7} {}s", v.status, v.age),
                })
                .collect();
            let filter_name = match filter {
                1 => i18n::t("jobs_filter_running"),
                2 => i18n::t("jobs_filter_watchers"),
                _ => i18n::t("jobs_filter_all"),
            };
            self.menu = Some(Menu {
                title: format!("{} \u{b7} {}", i18n::t("jobs_title"), filter_name),
                items,
                sel,
                hint: i18n::t("jobs_hint").into(),
                preview: preview.clone(),
            });
            self.draw();
            let k = match self.read_key_timeout() {
                Some(k) => k,
                None => continue,
            };
            if k == 0x1b {
                match self.read_key_after_esc() {
                    Some(b'[') => match self.read_key_timeout() {
                        Some(b'A') => {
                            sel = sel.saturating_sub(1);
                            preview = None;
                        }
                        Some(b'B') => {
                            sel = (sel + 1).min(views.len() - 1);
                            preview = None;
                        }
                        _ => {}
                    },
                    _ => {
                        self.menu = None;
                        self.draw();
                        return;
                    }
                }
                continue;
            }
            if views.is_empty() && !matches!(k, b'f' | b'F' | 0x1b | 0x03) {
                continue;
            }
            match k {
                0x03 | b'q' => {
                    self.menu = None;
                    self.draw();
                    return;
                }
                b'f' | b'F' => {
                    filter = (filter + 1) % 3;
                    sel = 0;
                    preview = None;
                }
                b's' | b'S' => {
                    let id = views[sel].id.clone();
                    let text = crate::bg::output_of(&id, 200);
                    let path = crate::config::tmp_dir().join(format!(
                        "{}-output.txt",
                        id.to_ascii_lowercase().replace(' ', "-")
                    ));
                    match std::fs::write(&path, &text) {
                        Ok(()) => {
                            let home = std::env::var("HOME").unwrap_or_default();
                            let mut shown = path.display().to_string();
                            if !home.is_empty() && shown.starts_with(&home) {
                                shown = format!("~{}", &shown[home.len()..]);
                            }
                            self.menu = None;
                            self.push(&i18n::tf("ui_saved_job", &[&id, &shown]), K_DIM);
                            return;
                        }
                        Err(e) => self.push(&i18n::tf("ui_save_failed", &[&e.to_string()]), K_ERR),
                    }
                }
                b'x' | b'X' => {
                    let id = views[sel].id.clone();
                    let note = match crate::bg::stop(&id) {
                        Ok(status) => format!("{id}: {status}"),
                        Err(e) => format!("{id}: {e}"),
                    };
                    self.menu = None;
                    self.push(&note, K_DIM);
                    preview = None;
                }
                0x0d | 0x0a => {
                    let id = views[sel].id.clone();
                    let text = crate::bg::output_of(&id, 200);
                    let rows = self.artifact_body_rows();
                    let cols = self.cols;
                    self.menu = None;
                    self.artifact = Some(Artifact::from_plain(
                        format!("{id} output"),
                        &text,
                        rows,
                        cols,
                    ));
                    self.draw();
                    return;
                }
                _ => {}
            }
        }
    }

    /// Record a submitted line for ↑ recall (never secrets like /key).
    fn history_push(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with("/key") {
            return;
        }
        if self.history.last().map(|l| l == line).unwrap_or(false) {
            return;
        }
        self.history.push(line.to_string());
        if self.history.len() > 60 {
            self.history.remove(0);
        }
        self.hist_pos = None;
        self.hist_draft.clear();
        // Persist across restarts (never API keys).
        let text = format!("{}\n", self.history.join("\n"));
        let path = crate::config::history_path();
        let _ = std::fs::write(&path, text);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }

    fn hist_set(&mut self, text: &str) {
        self.input = text.chars().collect();
        self.cursor = self.input.len();
        self.palette_sel = 0;
    }

    fn history_recall(&mut self, older: bool) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let cur = match self.hist_pos {
            None => {
                if !older {
                    return false;
                }
                self.hist_draft = self.input.iter().collect();
                self.history.len() - 1
            }
            Some(p) => {
                if older {
                    if p == 0 { return true; }
                    p - 1
                } else if p + 1 < self.history.len() {
                    p + 1
                } else {
                    self.hist_pos = None;
                    let draft = self.hist_draft.clone();
                    self.hist_set(&draft);
                    return true;
                }
            }
        };
        self.hist_pos = Some(cur);
        let text = self.history[cur].clone();
        self.hist_set(&text);
        true
    }

    /// CtrlQ: inspect/edit queued messages while a turn is running.
    /// Blocks the stream pump while open (the API channel buffers meanwhile).
    fn queue_panel_modal(&mut self) -> Option<String> {
        if self.queue.is_empty() {
            return None;
        }
        let mut sel = 0usize;
        loop {
            let mut out = String::with_capacity(2048);
            out.push_str("\x1b[?25l\x1b[2J\x1b[H\x1b[1;36m");
            out.push_str(&fit(&format!(" {}", i18n::t("queue_title")), self.cols));
            out.push_str("\x1b[0m\x1b[K\r\n");
            let rows = self.rows.saturating_sub(2).max(1);
            let start = if sel >= rows { sel + 1 - rows } else { 0 };
            for i in 0..rows {
                let idx = start + i;
                if let Some(item) = self.queue.get(idx) {
                    let selected = idx == sel;
                    let mark = if selected { "\u{25b8}" } else { " " };
                    let line = format!("{mark} {}. {}", idx + 1, item);
                    if selected {
                        out.push_str("\x1b[7m");
                    } else {
                        out.push_str("\x1b[0;37m");
                    }
                    // Pad to the panel width so the selection highlight and
                    // the row background span the whole line; ellipsise edits
                    // that are longer than the panel instead of cutting words.
                    let mut shown = fit_ellipsis(&line, self.cols);
                    shown.push_str(&" ".repeat(self.cols.saturating_sub(dw(&shown))));
                    out.push_str(&shown);
                    out.push_str("\x1b[0m\x1b[K\r\n");
                } else {
                    out.push_str("\x1b[K\r\n");
                }
            }
            out.push_str("\x1b[90m");
            out.push_str(&fit(&if self.queue.is_empty() { i18n::t("queue_empty") } else { i18n::t("queue_hint") }, self.cols));
            out.push_str("\x1b[0m\x1b[K");
            {
                use std::io::Write as _;
                let mut o = std::io::stdout();
                let _ = o.write_all(out.as_bytes());
                let _ = o.flush();
            }
            let k = match self.read_key_timeout() {
                Some(k) => k,
                None => continue,
            };
            if k == 0x1b {
                match self.read_key_after_esc() {
                    Some(b'[') => match self.read_key_timeout() {
                        Some(b'A') => sel = sel.saturating_sub(1),
                        Some(b'B') => sel = (sel + 1).min(self.queue.len().saturating_sub(1)),
                        _ => {}
                    },
                    _ => return None,
                }
                continue;
            }
            match k {
                0x03 => return None,
                b'x' | b'X' | 0x7f => {
                    if !self.queue.is_empty() {
                        self.queue.remove(sel);
                        if sel >= self.queue.len() {
                            sel = self.queue.len().saturating_sub(1);
                        }
                    }
                    if self.queue.is_empty() {
                        return None;
                    }
                }
                0x0d | 0x0a => {
                    if !self.queue.is_empty() {
                        return Some(self.queue.remove(sel));
                    }
                    return None;
                }
                _ => {}
            }
        }
    }

    /// Like `edit`, but a bare Enter submits an empty line: prompts with a
    /// sensible default ("keep deepseek-flash", "official base URL") should not
    /// force the user to type the default out.
    fn edit_allow_empty(&mut self) -> Option<String> {
        self.allow_empty = true;
        let r = self.edit();
        self.allow_empty = false;
        r
    }

    /// returns Some(line) when the user submits

    /// Ask once whether to install the update the background check found.
    /// Yes downloads/verifies/installs and re-execs the new binary; No/Esc
    /// dismisses it for this session (`/update` can re-offer at any time).
    fn offer_update(&mut self) {
        let Some(info) = self.update_offer.clone() else { return };
        self.startup_notice = None;
        let mut sel = 0usize;
        let target_label = format!(
            "{} ({})",
            info.version,
            crate::update::short_sha(&info.sha256)
        );
        let current_label = format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            crate::update::short_sha(&info.local_sha256)
        );
        loop {
            let items = vec![
                MenuItem {
                    label: i18n::t("ui_update_yes").to_string(),
                    value: String::new(),
                },
                MenuItem {
                    label: i18n::t("ui_update_no").to_string(),
                    value: String::new(),
                },
            ];
            let preview = Some(vec![
                i18n::tf("ui_update_target", &[&target_label, info.channel.as_str()]),
                i18n::tf("ui_update_current", &[&current_label]),
                format!("{}: {}", i18n::t("ui_update_asset_label"), info.asset),
            ]);
            self.menu = Some(Menu {
                title: i18n::t("ui_update_title").into(),
                items,
                sel,
                hint: i18n::t("ui_update_hint").into(),
                preview,
            });
            self.draw();
            let k = match self.read_key_timeout() {
                Some(k) => k,
                None => continue,
            };
            if k == 0x1b {
                match self.read_key_after_esc() {
                    Some(b'[') => match self.read_key_timeout() {
                        Some(b'A') => sel = sel.saturating_sub(1),
                        Some(b'B') => sel = (sel + 1).min(1),
                        _ => {}
                    },
                    _ => {
                        self.decline_update();
                        return;
                    }
                }
                continue;
            }
            match k {
                b'y' | b'Y' => {
                    self.install_update(&info);
                    return;
                }
                b'n' | b'N' | 0x03 => {
                    self.decline_update();
                    return;
                }
                0x0d | 0x0a => {
                    if sel == 0 {
                        self.install_update(&info);
                    } else {
                        self.decline_update();
                    }
                    return;
                }
                _ => {}
            }
        }
    }

    fn decline_update(&mut self) {
        self.menu = None;
        self.update_offer = None;
        self.push(i18n::t("ui_update_skipped"), K_DIM);
        self.draw();
    }

    /// Download, verify and install the offered build, then re-exec.
    fn install_update(&mut self, info: &crate::update::UpdateInfo) {
        self.menu = None;
        self.update_offer = None;
        self.busy = true;
        self.live = Some((1, i18n::t("ui_update_downloading").to_string()));
        self.draw();
        let result = crate::update::install(info, |stage| {
            let key = match stage {
                "verifying" => "ui_update_verifying",
                "installing" => "ui_update_installing",
                _ => "ui_update_downloading",
            };
            self.live = Some((1, i18n::t(key).to_string()));
            self.draw();
        });
        self.busy = false;
        self.live = None;
        match result {
            Ok(()) => {
                self.push(&i18n::tf("ui_update_done", &[&info.version]), K_NORM);
                self.draw();
                std::thread::sleep(std::time::Duration::from_millis(900));
                crate::update::restart_now();
            }
            Err(e) => {
                self.push(&i18n::tf("ui_update_failed", &[&e]), K_ERR);
                self.draw();
            }
        }
    }

    fn edit(&mut self) -> Option<String> {
        loop {
            self.draw();
            if self.qr.is_some() {
                match self.read_key_timeout() {
                    Some(0x1b) | Some(0x0d) | Some(0x0a) | Some(0x03) | Some(b'q') => {
                        close_qr(self);
                    }
                    _ => {}
                }
                continue;
            }
            if self.artifact.is_some() {
                self.artifact_keystroke();
                continue;
            }
            if self.update_offer.is_some() {
                self.offer_update();
                continue;
            }
            let ev = match self.read_key_event() {
                Some(e) => e,
                None => {
                    if let Some(mut f) = self.on_tick.take() {
                        f(self);
                        self.on_tick = Some(f);
                    }
                    self.anim_tick();
                    // A watcher line is a model turn even though the user did
                    // not type anything: hand it to the main loop.
                    if !self.auto_queue.is_empty() {
                        let label = self.auto_queue[0].1.clone();
                        let text = self
                            .auto_queue
                            .drain(..)
                            .map(|(line, _)| line)
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.auto_label = Some(label);
                        return Some(text);
                    }
                    continue;
                }
            };
            // Dismiss the one-time startup version banner on first interaction.
            self.startup_notice = None;
            // Navigation keys are handled before the byte dispatch: they are
            // decoded sequences, not characters.
            let b = match ev {
                KeyEvent::Char(b) => b,
                KeyEvent::Esc => 0x1b,
                KeyEvent::ShiftEnter => 0x0a,
                KeyEvent::Up => {
                    let n = self.palette_items().len();
                    if n > 0 {
                        self.palette_sel = self.palette_sel.saturating_sub(1);
                    } else if self.input.is_empty() && self.history_recall(true) {
                        // recalled history, nothing else to do
                    } else {
                        self.scroll_view(true, 5);
                    }
                    continue;
                }
                KeyEvent::Down => {
                    let n = self.palette_items().len();
                    if n > 0 {
                        self.palette_sel = (self.palette_sel + 1).min(n - 1);
                    } else if self.input.is_empty()
                        && crate::bg::has_any()
                        && !self.history_recall(false)
                    {
                        // Down on an empty prompt opens the background-task
                        // manager.
                        self.jobs_panel();
                    } else if self.hist_pos.is_some() {
                        self.history_recall(false);
                    } else {
                        self.scroll_view(false, 5);
                    }
                    continue;
                }
                KeyEvent::Left => {
                    self.cursor = self.cursor.saturating_sub(1);
                    continue;
                }
                KeyEvent::Right => {
                    self.cursor = (self.cursor + 1).min(self.input.len());
                    continue;
                }
                KeyEvent::Home => {
                    if self.input.is_empty() && self.scroll > 0 {
                        self.scroll_view(true, usize::MAX);
                    } else {
                        self.cursor = 0;
                    }
                    continue;
                }
                KeyEvent::End => {
                    if self.input.is_empty() && self.scroll > 0 {
                        self.scroll_view(false, usize::MAX);
                    } else {
                        self.cursor = self.input.len();
                    }
                    continue;
                }
                KeyEvent::PageUp => {
                    let page = self.body_rows().max(1);
                    self.scroll_view(true, page);
                    continue;
                }
                KeyEvent::PageDown => {
                    let page = self.body_rows().max(1);
                    self.scroll_view(false, page);
                    continue;
                }
                KeyEvent::Delete => {
                    if self.cursor < self.input.len() {
                        self.input.remove(self.cursor);
                    }
                    continue;
                }
            };
            match b {
                0x03 => return None,                       // Ctrl-C
                0x04 => return None,                       // Ctrl-D
                0x0f => {
                    // Ctrl-O: recent tool output viewer.
                    tool_output_panel(self);
                    continue;
                }
                0x11 => {
                    // Ctrl-Q: edit queued messages (normally only while busy).
                    if let Some(text) = self.queue_panel_modal() {
                        self.input = text.chars().collect();
                        self.cursor = self.input.len();
                    }
                    continue;
                }
                0x01 => self.cursor = 0,                   // Ctrl-A
                0x05 => self.cursor = self.input.len(),    // Ctrl-E
                0x17 => {
                    // Ctrl-W: delete the word before the cursor.
                    while self.cursor > 0 && self.input[self.cursor - 1].is_whitespace() {
                        self.cursor -= 1;
                        self.input.remove(self.cursor);
                    }
                    while self.cursor > 0 && !self.input[self.cursor - 1].is_whitespace() {
                        self.cursor -= 1;
                        self.input.remove(self.cursor);
                    }
                }
                0x0d | 0x0a => {
                    // Enter on an open palette: run a no-argument command, or
                    // complete a command that expects input.
                    let pal = self.palette_items();
                    if !pal.is_empty() {
                        let i = self.palette_sel.min(pal.len() - 1);
                        let typed: String = self.input.iter().collect();
                        let typed = typed.trim_end();
                        // An exactly typed command wins over the highlighted
                        // row, even when a fuzzy description match sorts first.
                        let chosen = pal
                            .iter()
                            .find(|(name, _, _, _)| typed == format!("/{name}"))
                            .cloned()
                            .unwrap_or_else(|| pal[i].clone());
                        let (name, _desc, hint, _cat) = chosen;
                        // A hint in [] is optional, <..> is required: only the
                        // latter completes with a space instead of running.
                        let needs_arg = hint.map(|h| h.starts_with('<')).unwrap_or(false);
                        let exact = typed == format!("/{name}");
                        if !needs_arg || exact {
                            let cmd = format!("/{name}");
                            self.input.clear();
                            self.cursor = 0;
                            return Some(cmd);
                        }
                        self.input.clear();
                        self.cursor = 0;
                        for ch in format!("/{name} ").chars() {
                            self.input.push(ch);
                        }
                        self.cursor = self.input.len();
                        continue;
                    }
                    if self.input.is_empty() {
                        if self.allow_empty {
                            self.input.clear();
                            self.cursor = 0;
                            return Some(String::new());
                        }
                        continue;
                    }
                    let line: String = self.input.iter().collect();
                    self.input.clear();
                    self.cursor = 0;
                    return Some(line);
                }
                0x7f | 0x08 => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                        self.input.remove(self.cursor);
                    }
                }
                0x15 => {
                    self.input.clear();
                    self.cursor = 0;
                }
                0x1b => {
                    // A lone Esc (read_key_event consumed any key sequence).
                    // Double Esc within 600 ms opens the rewind picker (the
                    // physical reset button maps to this gesture until a
                    // device-side GPIO hook exists).
                    let now = std::time::Instant::now();
                    let dbl = self
                        .last_esc
                        .map(|t| now.duration_since(t).as_millis() < 600)
                        .unwrap_or(false);
                    self.last_esc = Some(now);
                    if dbl {
                        self.last_esc = None;
                        return Some("/rewind".to_string());
                    }
                    crate::voice::request_stop();
                    pet_note(self, pet::Ev::Esc);
                    continue;
                }
                0x09 => {
                    self.voice_start_empty = Some(self.input.is_empty());
                    if self.voice_input() {
                        let line: String = self.input.iter().collect();
                        self.input.clear();
                        self.cursor = 0;
                        return Some(line);
                    }
                }
                0x20..=0x7e => {
                    if self.input.len() < MAX_INPUT {
                        self.input.insert(self.cursor, b as char);
                        self.cursor += 1;
                        self.palette_sel = 0;
                    }
                }
                0x80..=0xff => self.utf8_push(b),
                _ => {}
            }
        }
    }
}

/// Drop SGR/CSI sequences, for text that carries its own colour codes.
/// Wrap a single line for a narrow panel (approval arguments).
/// Tool-call card body: every argument on its own indented line, values longer
/// than the panel folded across rows, capped at `max_rows` so one call cannot
/// swallow the 12-row screen. `usize::MAX` = no cap (the CtrlO viewer).
fn expand_args(raw: &str, cols: usize, max_rows: usize) -> Vec<String> {
    let Some(crate::json::J::Obj(pairs)) = crate::json::parse(raw) else {
        return Vec::new();
    };
    let width = cols.saturating_sub(1).max(8);
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;
    'outer: for (key, val) in pairs {
        let text = match &val {
            crate::json::J::Str(s) => s.clone(),
            other => other.dump(),
        };
        let mut first = true;
        for line in text.split('\n') {
            let head = if first {
                format!("  {key}: ")
            } else {
                "    ".to_string()
            };
            let mut wrapped = Vec::new();
            Scr::wrap_into(&mut wrapped, &format!("{head}{line}"), width);
            if wrapped.is_empty() {
                out.push(head.trim_end().to_string());
            }
            for w in wrapped {
                if out.len() >= max_rows {
                    truncated = true;
                    break 'outer;
                }
                out.push(w);
            }
            first = false;
        }
    }
    if truncated {
        out.push(format!("  \u{2026} more: CtrlO"));
    }
    out
}

fn wrap(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    Scr::wrap_into(&mut out, text, PANEL_COLS.saturating_sub(2));
    out
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if let Some(n) = it.next() {
                if n == '[' {
                    for d in it.by_ref() {
                        if d.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// At a 6x13 cell a full-width `，。、：；` glyph is a lone 2-3 px speck, which
/// reads as noise around Chinese text. Substitute the ASCII forms for display:
/// the stored text is untouched, only what is painted changes.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{ff0c}' | '\u{3001}' => out.push(','),
            '\u{3002}' => out.push('.'),
            '\u{ff1a}' => out.push(':'),
            '\u{ff1b}' => out.push(';'),
            '\u{ff01}' => out.push('!'),
            '\u{ff1f}' => out.push('?'),
            '\u{ff08}' => out.push('('),
            '\u{ff09}' => out.push(')'),
            '\u{3010}' => out.push('['),
            '\u{3011}' => out.push(']'),
            '\u{201c}' | '\u{201d}' => out.push('"'),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{300a}' => out.push('<'),
            '\u{300b}' => out.push('>'),
            '\u{ff0d}' => out.push('-'),
            '\u{ff5e}' => out.push('~'),
            '\u{3000}' => out.push(' '),
            '\u{2026}' => out.push_str("..."),
            '\u{00b7}' => out.push('|'),
            _ => out.push(c),
        }
    }
    out
}

/// `1. text` -> ("1", "text")
fn numbered(t: &str) -> Option<(String, &str)> {
    let mut digits = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() && digits.len() < 3 {
            digits.push(c);
        } else if c == '.' && !digits.is_empty() {
            let rest = &t[digits.len() + 1..];
            if let Some(r) = rest.strip_prefix(' ') {
                return Some((digits, r));
            }
            return None;
        } else {
            return None;
        }
    }
    None
}

/// Drop inline markup the 6x13 font cannot render: **bold**, `code`,
/// [text](url) and stray emphasis markers.
/// Drop inline Markdown emphasis so the transcript reads as plain text, while
/// keeping characters that only look like markers: `snake_case` identifiers and
/// `2*3` are content, not emphasis (this used to print app names as
/// `batteryviewer` in creator mode).
fn strip_inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' | '_' | '`' => {
                if c == '`' {
                    i += 1;
                    continue;
                }
                // Run of identical markers, e.g. **bold** / __bold__.
                let mut n = 1usize;
                while i + n < chars.len() && chars[i + n] == c {
                    n += 1;
                }
                let prev = if i == 0 { ' ' } else { chars[i - 1] };
                let next = if i + n < chars.len() { chars[i + n] } else { ' ' };
                let prev_word = prev.is_alphanumeric();
                let next_word = next.is_alphanumeric();
                let keep = if c == '_' {
                    // Intraword underscore is part of the identifier.
                    prev_word && next_word
                } else if n == 1 {
                    // Multiplicative `a * b` / `2*3`, and a lone bullet-ish `*`.
                    prev_word == next_word
                } else {
                    false
                };
                if keep {
                    for _ in 0..n {
                        out.push(c);
                    }
                }
                i += n;
                continue;
            }
            '[' => {
                let mut j = i + 1;
                let mut label = String::new();
                let mut depth = 0;
                let mut closed = false;
                while j < chars.len() {
                    let d = chars[j];
                    j += 1;
                    if d == '[' {
                        depth += 1;
                    }
                    if d == ']' {
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                        depth -= 1;
                    }
                    label.push(d);
                }
                if closed && j < chars.len() && chars[j] == '(' {
                    j += 1;
                    let mut url = String::new();
                    while j < chars.len() {
                        if chars[j] == ')' {
                            j += 1;
                            break;
                        }
                        url.push(chars[j]);
                        j += 1;
                    }
                    out.push_str(&label);
                    out.push_str(" (");
                    out.push_str(&url);
                    out.push(')');
                } else {
                    out.push('[');
                    out.push_str(&label);
                    if closed {
                        out.push(']');
                    }
                }
                i = j;
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod inline_tests {
    use super::{claim_terminal, strip_inline, tty_lock_name};

    #[test]
    fn the_prompt_teaches_planning_and_per_step_reports() {
        // The model was rarely calling todo_write/state_report: both must be
        // spelled out in the prompt, aligned with DSH's todo contract.
        let cfg = crate::config::Config::default();
        let prompt = super::build_system_prompt(
            &cfg,
            &[],
            crate::config::Capabilities {
                web_tools: true,
                speech: true,
            },
        );
        for needle in [
            "PROGRESS ROW",
            "state_report ONCE at the start of every step",
            "PLANNING",
            "call todo_write BEFORE you start",
            "exactly one item 'in_progress'",
            "Mark each todo 'completed' as soon as it is done",
        ] {
            assert!(prompt.contains(needle), "prompt misses {needle:?}");
        }
    }

    #[test]
    fn prompt_covers_every_registered_tool_and_respects_capabilities() {
        let caps_all = crate::config::Capabilities {
            web_tools: true,
            speech: true,
        };
        let mut cfg = crate::config::Config::default();
        let normal_prompt = super::build_system_prompt(&cfg, &[], caps_all);
        for def in super::build_registry(caps_all, false, &[]) {
            if !def.name.starts_with("mcp__") {
                assert!(
                    normal_prompt.contains(def.name),
                    "normal prompt misses tool {:?}",
                    def.name
                );
            }
        }
        assert!(!normal_prompt.contains("CREATOR MODE"));
        assert!(!normal_prompt.contains("CAPABILITIES"));

        cfg.creator = true;
        let skill = crate::skills::Skill {
            name: "demo-skill".into(),
            description: "demo".into(),
            body: "body".into(),
            dir: std::path::PathBuf::from("/tmp/demo-skill"),
            origin: "model".into(),
            version: 1,
            updated: String::new(),
        };
        let creator_prompt = super::build_system_prompt(&cfg, &[skill], caps_all);
        for def in super::build_registry(caps_all, true, &[]) {
            if !def.name.starts_with("mcp__") {
                assert!(
                    creator_prompt.contains(def.name),
                    "creator prompt misses tool {:?}",
                    def.name
                );
            }
        }
        // Static prefix precedes turn-volatile catalog and mode sections.
        let lang_pos = creator_prompt.find("\nLANGUAGE\n").unwrap();
        let skills_pos = creator_prompt.find("\nSKILLS\n").unwrap();
        let creator_pos = creator_prompt.find("\nCREATOR MODE").unwrap();
        assert!(lang_pos < skills_pos && skills_pos < creator_pos);

        let caps_none = crate::config::Capabilities {
            web_tools: false,
            speech: false,
        };
        cfg.creator = false;
        let bare = super::build_system_prompt(&cfg, &[], caps_none);
        assert!(bare.contains("CAPABILITIES"));
        assert!(!bare.contains("WEB & DOCUMENTS"));
        assert!(!bare.contains("\nVOICE\n"));

        // disabled_tools removes tools from the registry and notifies the prompt.
        let disabled = vec!["speak".to_string(), "cron".to_string()];
        let filtered = super::build_registry(caps_all, false, &disabled);
        assert!(!filtered.iter().any(|t| t.name == "speak" || t.name == "cron"));
        cfg.disabled_tools = disabled;
        let with_disabled = super::build_system_prompt(&cfg, &[], caps_all);
        assert!(with_disabled.contains("CAPABILITIES"));
        assert!(with_disabled.contains("speak, cron"));
    }

    #[test]
    fn instances_on_different_terminals_are_allowed() {
        let dir = std::env::temp_dir().join(format!(
            "dst-tty-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let alive = |pid: u32| pid != 0;
        assert!(claim_terminal(&dir, "pts/1", 111, &alive));
        assert!(
            claim_terminal(&dir, "pts/2", 222, &alive),
            "a second instance on another terminal must be allowed"
        );
        assert!(
            !claim_terminal(&dir, "pts/1", 333, &alive),
            "a second instance on the same terminal must be refused"
        );
        assert!(
            claim_terminal(&dir, "pts/3", 444, &|_| false),
            "a stale lock (dead pid) must be taken over"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tty_lock_names_are_safe_file_names() {
        assert_eq!(tty_lock_name("pts/3"), "pts_3");
        assert_eq!(tty_lock_name("ttys004"), "ttys004");
        assert_eq!(tty_lock_name(""), "tty");
        assert_eq!(tty_lock_name("a b/c"), "a_b_c");
    }

    #[test]
    fn keeps_identifiers_and_math() {
        assert_eq!(strip_inline("system/battery_viewer"), "system/battery_viewer");
        assert_eq!(strip_inline("snake_case_name"), "snake_case_name");
        assert_eq!(strip_inline("3 * 4 = 12"), "3 * 4 = 12");
        assert_eq!(strip_inline("2*3"), "2*3");
    }

    #[test]
    fn drops_emphasis_and_code_markers() {
        assert_eq!(strip_inline("**bold** text"), "bold text");
        assert_eq!(strip_inline("_italic_ text"), "italic text");
        assert_eq!(strip_inline("`code` here"), "code here");
        assert_eq!(strip_inline("__bold__"), "bold");
    }

    #[test]
    fn keeps_links_readable() {
        assert_eq!(strip_inline("see [docs](https://x.y)"), "see docs (https://x.y)");
        assert_eq!(strip_inline("[unclosed"), "[unclosed");
    }
}

/// Display width in terminal cells (CJK counts as two).
fn dw(s: &str) -> usize {
    s.chars().map(Scr::cw).sum()
}

/// Splash hint line under the 9-row sprite. If the model name pushes the
/// line past the panel width, shorten the model instead of cutting the key
/// hints after it (`... | hold Tab to talk | /help` stays visible).
fn splash_hint(model: &str, cols: usize) -> String {
    let full = format!("  {}", i18n::tf("hints", &[model]));
    if dw(&full) <= cols {
        return full;
    }
    let suffix = i18n::tf("hints", &[""]);
    let room = cols.saturating_sub(2 + dw(&suffix));
    if room >= 8 {
        let short = fit_dots(model, room);
        return format!("  {}", i18n::tf("hints", &[&short]));
    }
    fit_dots(&full, cols)
}

/// Truncate to `cols` cells with an ASCII `...` marker.
///
/// Transcript helpers run text through `tidy`, which expands a single `…`
/// into three cells; a line that fit with `fit_ellipsis` then re-wrapped and
/// split its last word (the blocked-reason preview is the live example).
fn fit_dots(s: &str, cols: usize) -> String {
    if dw(s) <= cols {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = Scr::cw(ch);
        if w + cw > cols.saturating_sub(3) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push_str("...");
    out
}

/// Like `fit`, but marks visible truncation with an ellipsis.
fn fit_ellipsis(s: &str, cols: usize) -> String {
    if dw(s) <= cols {
        return s.to_string();
    }
    let mut out = fit(s, cols.saturating_sub(1));
    out.push('\u{2026}');
    out
}

fn fit(s: &str, cols: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = Scr::cw(ch);
        if w + cw > cols {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// Project the persistent config onto the per-turn API client config.
fn api_cfg(c: &config::Config) -> Cfg {
    Cfg {
        base: c.base.clone(),
        key: c.api_key.clone(),
        token: c.dsh_token.clone(),
        model: c.model.clone(),
        max_tokens: c.max_tokens,
        effort: c.effort.clone(),
    }
}

/// Refresh the context-window figure only when the endpoint or model changed.
fn refresh_ctx_window(scr: &mut Scr, cfg: &Cfg) {
    let key = format!("{}|{}", cfg.base, cfg.model);
    if key == scr.ctx_for {
        return;
    }
    scr.ctx_window = crate::api::context_window(cfg).unwrap_or(0);
    scr.ctx_for = key;
}

fn term_size() -> (usize, usize) {
    // Probe the pty this process actually owns first (plain `stty size` reads
    // stdin), then /dev/tty. Whatever comes back is clamped to the panel:
    // 320x170 / 6x13 = 53 columns x 13 rows. Sizing to a larger default is
    // what made long lines run past the edge and soft-wrap.
    let probes: [&[&str]; 3] = [
        &["size"],
        &["-F", "/dev/tty", "size"],
        &["-f", "/dev/tty", "size"],
    ];
    for probe in probes {
        if let Ok(o) = std::process::Command::new("stty").args(probe).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            let mut it = s.split_whitespace();
            if let (Some(r), Some(c)) = (it.next(), it.next()) {
                if let (Ok(rows), Ok(cols)) = (r.parse::<usize>(), c.parse::<usize>()) {
                    if rows > 0 && cols > 0 {
                        return (rows.min(13), cols.min(53));
                    }
                }
            }
        }
    }
    (12, 53)
}

/// Run `stty` against the controlling terminal. GNU stty takes `-F`, the BSD
/// stty on macOS takes `-f`; try both so the host simulator really exercises
/// the same raw/poll modes the device gets (otherwise the simulator's own
/// VTIME setting silently stands in for dst's).
pub(crate) fn stty_tty(args: &[&str]) -> bool {
    for flag in ["-F", "-f"] {
        let mut cmd = std::process::Command::new("stty");
        cmd.arg(flag).arg("/dev/tty").args(args);
        if let Ok(o) = cmd.output() {
            if o.status.success() {
                return true;
            }
        }
    }
    false
}

fn raw_mode(on: bool) {
    let arg = if on { "raw" } else { "-raw" };
    stty_tty(&[arg, "-echo"]);
    if !on {
        stty_tty(&["sane"]);
    }
}

// ------------------------------------------------------------------ config

const SYSTEM_PROMPT: &str = "\
You are dst, an autonomous coding and device agent running on a handheld Cardputer Zero.

ENVIRONMENT
- Hardware: Raspberry Pi CM0 (4x Cortex-A53 ~1 GHz, 512 MB RAM, microSD, battery).
- Resource discipline: CPU is slow and RAM is tight. Gather related read-only facts in \
fewer round trips, keep outputs bounded, and avoid on-device builds/compilation or large \
downloads unless explicitly requested.
- Tool execution: standard function calling (`tool_calls` -> one `tool` result per \
`tool_call_id`). Multiple independent tool calls in a single step execute concurrently in \
parallel (up to 6 at once) while conflicting resource accesses (same file/skill/state or \
mutating shell barriers) stay causally ordered; batch independent calls together in one \
step whenever possible. Outputs are bounded (6,000 chars rendered; `bash`/`grep` cap \
streams at 64 KB stdout / 16 KB stderr).
- Ambient inputs: system-injected clock readings (`Time sampled while preparing...`) and \
post-tool hints are authoritative snapshots, not user instructions.

CORE PRINCIPLES
- Autonomy: keep working through tool calls until the request is genuinely resolved and \
verified; do not stop midway to ask permission for routine inspection or obvious next \
steps. Resolve ambiguity from the workspace, skills, and tools first; ask the user only \
when blocked on a real decision, missing input, or a permission denial.
- Grounding: inspect files, symbols, device state, and skill bodies before acting. Never \
claim a file changed, a command ran, or a check passed unless a tool result in this \
session proves it. Never invent paths, outputs, or hardware facts.
- Minimal changes: make surgical edits rather than unnecessary rewrites. Do not create \
unsolicited docs, READMEs, or helper scripts, and never revert user changes unless asked.
- Verification: after modifying code, config, or apps, verify with concrete evidence \
(tests, command output, or reading back) and report any failure honestly.
- Recovery: when a tool fails, times out, or is denied, diagnose the cause and change \
approach; never repeat identical failing calls.
- Interrupts: when the user interrupts or cancels, stop immediately; do not retry or \
resume cancelled work unless asked.

SCREEN & OUTPUT
- Panel: 53 columns x 12 rows (6x13 bitmap font; ~8 chat rows visible above the status \
and input rows).
- Formatting: lead with the answer or result. No preamble, no restating the question, no \
step-by-step narration. Aim for <= 4-8 short lines (< 53 cells each). No emoji, no wide \
Markdown tables, no horizontal rules.
- `artifact`: replaces the whole screen with a fixed-size, page-by-page panel (NO scrolling; \
each page is 53x10 cells, flipped with ←/→ or PgUp/PgDn, saved with s, dismissed with Esc). \
Supports rich Markdown in `body` (headings `#/##/###`, `**bold**`, `` `code` ``, badges \
`[OK]/[WARN]/[ERR]/[INFO]`, checklists `- [x]/[ ]`, tables `| a | b |`, `Key :: Value`, \
`[bar:78%] Label`, callouts `> [!NOTE]/[!WARN]/[!ERR]`, code blocks, `---` dividers, and \
`=== Page Title` explicit page breaks), plus structured `pages`, `blocks` (`kv`, `table`, \
`bar`, `bars`, `spark`, `columns`, `box`, `callout`, `code`, `list`, `checklist`, `divider`, \
`heading`, `text`), `theme`, and `layout` (`auto`, `center`, `kv`, `columns`). Use ONLY for \
finished results that genuinely need structured room - never for progress notes or one-line \
answers.

TOOLS
Prefer dedicated tools over `bash` whenever one fits:
- `read_file`: read text files with 1-based line numbers (`offset`/`limit`); follow \
continuation hints instead of re-reading blindly. Always read a file before editing or \
overwriting it.
- `list_dir`, `glob`, `grep`: inspect directories, locate files by pattern, and search \
contents with extended regex instead of running `ls`, `find`, or `grep` in `bash`.
- `edit_file`: preferred for surgical in-place edits on existing source-sized files \
(<= 8 MB). Match `old_string` verbatim with enough context to be unique, or set \
`replace_all=true`.
- `write_file`: create new files or intentionally overwrite a file completely (creates \
parent dirs).
- `bash`: reserve for operations dedicated tools cannot perform (system/hardware probes, \
running scripts/tests, `git`, audio/display/network checks). Combine related read-only \
checks into one compact command, bound output (never `cat` huge logs), and use `timeout_s` \
or `job_start` for long commands.
- `read_image` (`{\"file_path\": \"...\"}`): inspect local PNG/JPEG/WebP/GIF images \
directly (<= 8 MB, vision-capable model required). User-attached images (`/image <path>`) \
also arrive as image blocks - answer directly from what is visible rather than asking for \
a description. Do not install image libraries, generate thumbnails, or open an image just \
to restate its filename. Each image costs up to ~1,000 tokens, so avoid unnecessary reads. \
If the model/endpoint rejects image blocks (400), say so plainly and offer `ocr` / `/ocr` \
or a vision-capable model.
- `skill`: when a task matches a package in the SKILLS catalog (including `hardware-info` \
before probing or writing code for this Cardputer Zero's hardware), call `skill` with its \
exact name before acting. Do not guess a skill's contents.
- `ask_user`: ask the user structured multiple-choice questions on the panel (single-select \
or `allow_multiple=true`; an 'Other...' custom text option is added automatically). Use it \
ONLY when genuinely blocked on a user decision instead of listing choices in chat prose; \
put a recommended option first with '(Recommended)' and keep prompts/labels concise for \
the 53x12 screen.

PROGRESS ROW
- Call state_report ONCE at the start of every step that runs tools (every assistant step \
whose reply includes tool calls), and again whenever the stage changes (e.g. \"reading \
/proc\", \"editing run.sh\", \"running tests\").
- One short line, 50 display cells at most (CJK counts as 2), no newlines, in the user's \
language; longer reports are rejected. State the current activity, not a conclusion. Never \
put secrets in it and do not repeat the same report back-to-back.
- The pinned top row stays visible while tool cards and text stream in and only changes on \
the next `state_report`; skipping it leaves a stale stage on screen.

PLANNING
- For multi-step work (more than a couple of steps, several tool calls, or work across \
files), call todo_write BEFORE you start: send the COMPLETE list of `{content, status}` \
items (`pending` | `in_progress` | `completed`), one todo per concrete step, keeping \
exactly one item 'in_progress'.
- Update the whole list as steps finish. Mark each todo 'completed' as soon as it is done. \
Skip it for trivial single-step requests. Use `todo_read` to inspect the current list.
- The list drives the `done/total` status badge and resets on the next turn: it is the \
plan for this turn, not durable storage. `todo_write` tracks the plan; `state_report` \
shows what you are doing right now.

JOBS, WATCHERS & SCHEDULING
- `job_start`, `job_list`, `job_output`, `job_wait`, `job_kill`: detached shell processes \
for long-running work within this session (scans, downloads, builds) so other calls can \
continue. Start independent jobs in parallel; kill them when done.
- `watcher_start`, `watcher_list`, `watcher_stop`: session-scoped background processes \
whose every stdout/stderr line arrives as a `[watcher <id>]` user message (live logs, \
servers, sensors). Stop when no longer needed.
- `cron` & `watch`: persistent triggers stored in `~/.dst/jobs.json` that fire as \
standalone turns when idle (`cron` on a 5-field schedule; `watch` when a file/directory \
mtime changes). Write self-contained prompts for `add`; inspect with `list` before `remove`.

PERMISSIONS
- Tool calls pass through the safeguard (`auto` / `strict` / `full`). Known-safe calls \
(read-only commands, web reads, workspace writes, harness tools) run directly. \
System-configuration changes (`sudo`, service control, package installs, writes under \
`/etc`, `/boot`, `/usr`, `/lib`, `/sys`, `/proc`, `/dev`, `/var`, destructive `rm`/`git`) \
and uncertain calls are checked by the safeguard in `auto` mode and require human approval \
in `strict` mode.
- Two-phase escalation in `auto` mode: when a call is blocked by the safeguard, you are \
notified first with the concrete block reason. Do NOT blindly retry the identical call. \
First judge whether the operation is genuinely necessary or if a safer workspace-local or \
read-only alternative accomplishes the goal. If it IS truly necessary, retry the exact \
same call with `request_user_approval: true` and `approval_reason` set to the prior block \
reason (plus a brief justification) so the TUI prompts the user directly (`[y] allow` / \
`[n] reject`). Never set `request_user_approval: true` preemptively before a call is \
blocked. If the user rejects the confirmation prompt (`rejected by the user`), accept the \
refusal without retrying and explain the outcome.

MEMORY
- `memory_read` / `memory_search`: inspect project (`./AGENTS.md`) and global \
(`~/.dst/AGENTS.md`) memory before changing conventions or when prior preferences matter.
- `memory_write`: persist short, factual, durable rules (`scope=project` for workspace \
conventions, `scope=global` for cross-project preferences; appends by default, \
`replace=true` overwrites). Never store secrets, credentials, or ephemeral task state.

SESSION
- This conversation is saved and shown in `/resume`. Call `rename` with a concise, \
concrete 1-line title (<= 60 chars) as soon as you understand the task, and update it \
whenever the task shifts substantially (new sub-task, different bug, next phase). Never \
use greetings or copy the raw user message.";

const WEB_TOOLS_GUIDANCE: &str = "\n\nWEB & DOCUMENTS\n\
- `search`: query the web for current or external facts instead of guessing (3-5 results \
usually fit this screen; keep `need_content=false` by default).\n\
- `fetch`: read one http(s) URL as Markdown after `search` when summaries are not enough.\n\
- `ocr`: extract text page-by-page from a local PDF, Word, PowerPoint, or image file \
(use `purpose=rich_layout_recognition` when tables or layout matter).";

const VOICE_GUIDANCE: &str = "\n\nVOICE\n\
- `speak`: text-to-speech via the device speaker ONLY when the user explicitly asks to be \
told aloud (e.g. \"read it to me\", \"念给我听\"). Never use it for progress, \
confirmations, or status; the screen is the primary channel.\n\
- Keep every spoken message <= 150 words (a CJK character counts as 1 word); longer input \
is rejected, so summarize aloud and leave detail in reply text.\n\
- The user speaks by long-pressing Tab and the transcript submits automatically; treat \
voice messages naturally and reply conversationally.";

/// Bounded conversation tail handed to a one-off `/btw` subagent: newest
/// messages last, at most 96 KB, never starting with an orphan tool result
/// (tool messages need their assistant tool_calls before them).
fn btw_tail(src: &[json::J]) -> Vec<json::J> {
    const MAX_CONTEXT_BYTES: usize = 96 * 1024;
    let mut start = src.len();
    let mut used = 0usize;
    while start > 1 {
        let len = src[start - 1].dump().len();
        if used + len > MAX_CONTEXT_BYTES {
            break;
        }
        used += len;
        start -= 1;
    }
    while start < src.len() {
        let role = src[start].get("role").and_then(|r| r.as_str());
        if role == Some("tool") {
            start += 1;
        } else {
            break;
        }
    }
    src[start..].to_vec()
}

/// A bounded context snapshot the busy-time `/btw` can use without borrowing
/// the live `msgs` (the turn holds a mutable borrow of it). Includes the
/// current turn's user prompt so mid-turn `/btw` knows what is being asked.
fn btw_snapshot(msgs: &[json::J], current_prompt: Option<&str>) -> jobs::Msgs {
    let base = msgs
        .first()
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let mut v = vec![harness::msg("system", base)];
    v.extend(btw_tail(msgs));
    if let Some(p) = current_prompt {
        let p = p.trim();
        if !p.is_empty() {
            v.push(harness::msg("user", p));
        }
    }
    std::rc::Rc::new(std::cell::RefCell::new(v))
}

/// The side-question contract appended to the live system prompt.
/// Does not duplicate the question (which `run_turn` appends as the user turn).
fn btw_contract(base: &str, in_flight: Option<&str>) -> String {
    let mut s = format!(
        "{base}\n\nSIDE QUESTION MODE\n\
You are a one-off, read-only subagent answering an ephemeral side question about \
this conversation.\n\
- Allowed: answer directly from the conversation above and read-only tools only \
(`read_file`, `read_image`, `list_dir`, `glob`, `grep`, `search`, `fetch`, \
`ocr`, `skill`, `memory_read`, `memory_search`, `todo_read`, `get_goal`, \
`job_list`, `job_output`, `watcher_list`).\n\
- Forbidden: never write, edit, run shell commands, mutate skills/apps/memory/\
todos/goals, start jobs/watchers/schedules, call `artifact`, `speak`, `ask_user`, \
`rename`, or `state_report`.\n\
- Output: rendered directly in a dedicated full-screen paginated panel (nothing \
is saved to the main conversation). Be direct, concise, and grounded — no \
preamble or \"based on the context\" filler. Short Markdown (bullets, `code`, \
`**bold**`, `Key :: Value`) is supported. If the answer is not available, state \
exactly what is missing."
    );
    if let Some(status) = in_flight {
        if !status.trim().is_empty() {
            s.push_str("\n\nIN-FLIGHT MAIN TURN (currently running in background):\n");
            s.push_str(status.trim());
        }
    }
    s
}

/// Context handed to a one-off `/btw` subagent: the current system prompt with
/// a side-question contract, a bounded tail of the conversation, then the
/// question (appended by `run_turn`). Nothing is written back to `msgs`.
fn btw_context(msgs: &jobs::Msgs, in_flight: Option<&str>) -> Vec<json::J> {
    let src = msgs.borrow();
    let base = src
        .first()
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let mut out = vec![harness::msg("system", &btw_contract(base, in_flight))];
    out.extend(btw_tail(&src));
    out
}

/// Live overlay UI for `/btw`: renders thinking, read-only tool activity, and
/// streamed answer directly inside the `/btw` panel while leaving the main
/// transcript completely untouched. Esc cancels immediately.
struct BtwUi<'a> {
    scr: &'a mut Scr,
    live_title: String,
    status: String,
    acc: String,
    note: Option<String>,
    interrupted: bool,
}

impl<'a> BtwUi<'a> {
    fn repaint(&mut self) {
        let rows = self.scr.artifact_body_rows();
        let cols = self.scr.cols;
        let body = if !self.acc.trim().is_empty() {
            self.acc.clone()
        } else if !self.status.is_empty() {
            format!("*{}*", self.status)
        } else {
            format!("*{}*", i18n::t("btw_thinking"))
        };
        let mut a = Artifact::from_markdown(&self.live_title, &body, rows, cols);
        a.theme = crate::artifact::Theme::Info;
        a.notice = Some(i18n::t("btw_cancel_hint").to_string());
        // Show the latest page while streaming so newly arrived lines are visible.
        a.page = a.total_pages().saturating_sub(1);
        self.scr.artifact = Some(a);
        self.scr.draw();
    }
}

impl<'a> harness::Ui for BtwUi<'a> {
    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::TurnStart => {
                self.scr.busy = true;
                self.repaint();
            }
            Ev::Delta(d) => {
                self.acc.push_str(&d);
                self.repaint();
            }
            Ev::Thinking { secs, .. } => {
                self.status = format!("{} {secs:.1}s", i18n::t("btw_thinking"));
                self.repaint();
            }
            Ev::ThinkingDone { .. } => {}
            Ev::Assistant(_) => {}
            Ev::ToolCall { name, .. } => {
                self.status = i18n::tf("btw_tool", &[&name]);
                self.repaint();
            }
            Ev::ToolResult { .. } => {
                self.status = i18n::t("btw_thinking").to_string();
                self.repaint();
            }
            Ev::Note(n) => self.note = Some(n),
            Ev::Info(n) => {
                if self.note.is_none() {
                    self.note = Some(n);
                }
            }
            _ => {}
        }
    }

    fn pump(&mut self) -> Action {
        loop {
            match self.scr.read_key_event_now() {
                Some(KeyEvent::Esc)
                | Some(KeyEvent::Char(0x03))
                | Some(KeyEvent::Char(0x04)) => {
                    self.interrupted = true;
                    return Action::Cancel;
                }
                Some(_) => {}
                None => break,
            }
        }
        Action::Continue
    }

    fn confirm(&mut self, _request: &crate::sandbox::Approval) -> bool {
        // Only read-only tools are registered; nothing may be approved.
        false
    }

    fn interrupted(&self) -> bool {
        self.interrupted
    }
}

/// Run one read-only side question and show the answer in a dedicated fixed-page
/// panel. The main transcript, conversation history, context ring, and session
/// are never modified.
fn run_btw(
    scr: &mut Scr,
    cfg: &Cfg,
    caps: config::Capabilities,
    msgs: &jobs::Msgs,
    question: &str,
    opts: &harness::RunOpts,
    in_flight: Option<&str>,
) {
    let reg = tools::view_registry(caps);
    let mut ctx = btw_context(msgs, in_flight);
    let mut btw_opts = opts.clone();
    // Ephemeral side question: never trigger compaction, never mutate global
    // time-context throttling, and never overwrite the main context ring.
    btw_opts.compact.enabled = false;
    btw_opts.context_time = false;
    btw_opts.update_context = false;
    btw_opts.max_steps = if btw_opts.max_steps == 0 {
        8
    } else {
        btw_opts.max_steps.min(8)
    };

    let prev_busy = scr.busy;
    let prev_live = scr.live.clone();
    let flat = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let live_title = i18n::tf("btw_live_title", &[&fit_dots(&flat, 40)]);
    let final_title = i18n::tf("btw_title", &[&fit_dots(&flat, 42)]);

    scr.busy = true;
    // The subagent polls the keyboard from `pump` (Esc cancels it), so stdin
    // must be non-blocking for its whole run — the idle editor leaves VMIN=1.
    poll_mode(true);
    let (report, note) = {
        let mut ui = BtwUi {
            scr: &mut *scr,
            live_title,
            status: i18n::t("btw_thinking").to_string(),
            acc: String::new(),
            note: None,
            interrupted: false,
        };
        ui.repaint();
        let report = harness::run_turn(cfg, &btw_opts, &mut ctx, &reg, question, &[], &mut ui);
        (report, ui.note.take())
    };
    poll_mode(prev_busy);
    scr.busy = prev_busy;
    scr.live = prev_live;

    let rows = scr.artifact_body_rows();
    let cols = scr.cols;
    if report.cancelled {
        // Clean cancel: dismiss the overlay without polluting the main transcript.
        scr.artifact = None;
    } else if report.text.trim().is_empty() {
        let why = note.unwrap_or_else(|| i18n::t("btw_empty").to_string());
        let mut a = Artifact::from_markdown(
            &final_title,
            &format!("> [!ERR] {}", i18n::tf("btw_failed", &[&why])),
            rows,
            cols,
        );
        a.theme = crate::artifact::Theme::Error;
        scr.artifact = Some(a);
    } else {
        let mut a = Artifact::from_markdown(&final_title, report.text.trim(), rows, cols);
        a.theme = crate::artifact::Theme::Info;
        scr.artifact = Some(a);
    }
    scr.draw();
}

/// `--once`: the same driver, printing events instead of drawing them.
struct OnceUi;

impl harness::Ui for OnceUi {
    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::Delta(d) => {
                print!("{d}");
                let _ = std::io::stdout().flush();
            }
            Ev::ThinkingDone { secs, chars } => {
                eprintln!("[thinking {secs:.1}s, {chars} chars]");
            }
            Ev::Assistant(_) => println!(),
            Ev::ToolCall { name, title, .. } => println!("[call] {name}: {title}"),
            Ev::ToolResult {
                name,
                content,
                is_error,
                ..
            } => {
                let tag = if is_error { "error" } else { "result" };
                println!("[{tag}:{name}] {}", first_lines(&content, 6));
            }
            Ev::Context(c) => println!("[context] {c}"),
            Ev::Compacting { tokens, messages } => {
                eprintln!("[compacting {messages} messages, {} tokens]", crate::compact::human_tokens(tokens));
            }
            Ev::CompactingDone { ok, note } => eprintln!("[compact {}] {note}", if ok { "ok" } else { "failed" }),
            Ev::Info(n) => eprintln!("[info] {n}"),
            Ev::Hint(n) => eprintln!("[hint] {n}"),
            Ev::Note(n) => eprintln!("[note] {n}"),
            _ => {}
        }
    }
    fn pump(&mut self) -> Action {
        Action::Continue
    }
    fn confirm(&mut self, request: &crate::sandbox::Approval) -> bool {
        // No keyboard in --once mode: strict mode fails closed.
        eprintln!(
            "[permission] strict mode cannot ask in --once mode; rejecting {} ({})",
            request.tool, request.reason
        );
        false
    }
}

fn run_once(cfg: &Cfg, reg: &[tool::ToolDef], system_prompt: &str, prompt: &str) {
    let mut msgs = vec![harness::msg("system", system_prompt)];
    let mut ui = OnceUi;
    harness::run_turn(cfg, &harness::RunOpts::default(), &mut msgs, reg, prompt, &[], &mut ui);
}

// ---------------------------------------------------------------- interactive

/// Front end for the terminal. Draws harness events, and keeps the keyboard
/// live while the model works: typed text is echoed, Enter queues the message
/// for after this turn, Esc cancels.
fn pet_note(scr: &mut Scr, ev: pet::Ev) {
    let Some(pet) = scr.pet.clone() else { return };
    let unlocked = pet.borrow_mut().on_event(ev);
    for id in unlocked {
        let name = pet_growth::achievement_name(&id);
        scr.push(&i18n::tf("pet_ach", &[&format!("{id} {name}")]), K_DIM);
    }
}

fn apply_pet(scr: &mut Scr, cfg: &mut config::Config, arg: &str) {
    let Some(pet) = scr.pet.clone() else { return };
    let lines = pet.borrow_mut().command(arg, cfg, true);
    for (text, kind) in lines {
        if !text.is_empty() {
            scr.push(&text, kind);
        }
    }
    scr.pet_idle_frame();
}

struct TermUi<'a> {
    scr: &'a mut Scr,
    acc: String,
    /// Incremental reasoning line buffer: reasoning is wrapped as it arrives
    /// instead of re-wrapping the whole accumulated text on every chunk (that
    /// was quadratic and made the panel lag behind the stream).
    think_cur: String,
    think_wrapped: Vec<String>,
    /// Messages typed while busy, injected at the next step boundary (steer).
    steer: Vec<String>,
    tools: std::collections::HashMap<String, (String, String, std::time::Instant, String)>,
    spin: usize,
    /// The user asked the whole turn to stop (Esc / voice interrupt).
    interrupted: bool,
    /// Config handle for commands that may run while the model is busy.
    cfg_rc: std::rc::Rc<std::cell::RefCell<config::Config>>,
    /// Bounded pre-turn context snapshot used by a mid-turn `/btw`.
    btw_ctx: jobs::Msgs,
}

impl<'a> TermUi<'a> {
    fn reset_think(&mut self) {
        self.think_cur.clear();
        self.think_wrapped.clear();
        self.scr.think_active = false;
        self.scr.think_lines.clear();
    }

    /// Move the line currently being wrapped into the finished-line buffer.
    fn think_flush(&mut self) {
        let line = self.think_cur.trim_end().to_string();
        self.think_cur.clear();
        if !line.trim().is_empty() {
            self.think_wrapped.push(line);
            if self.think_wrapped.len() > 8 {
                let drop = self.think_wrapped.len() - 8;
                self.think_wrapped.drain(0..drop);
            }
        }
    }

    /// Rebuild the visible tail from finished lines plus the line in progress.
    /// Chunks are *not* line breaks: committing on every delta was what made
    /// the reasoning panel show one short line per stream chunk.
    fn think_refresh(&mut self) {
        let mut shown: Vec<String> = self
            .think_wrapped
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|l| format!("  {l}"))
            .collect();
        if !self.think_cur.trim().is_empty() {
            shown.push(format!("  {}", self.think_cur.trim_end()));
        }
        if shown.len() > 4 {
            let drop = shown.len() - 4;
            shown.drain(0..drop);
        }
        self.scr.think_lines = shown;
        self.scr.think_active = !self.scr.think_lines.is_empty();
    }
}

impl<'a> harness::Ui for TermUi<'a> {
    fn run_app(
        &mut self,
        app: &crate::apps::App,
        timeout_s: u64,
        args: &str,
    ) -> Result<String, String> {
        let result = crate::apps::run_on_screen_args(app, timeout_s, args);
        self.scr.draw();
        result
    }

    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::TurnStart => pet_note(self.scr, pet::Ev::TurnStart),
            Ev::Delta(d) => {
                pet_note(self.scr, pet::Ev::Talk);
                self.acc.push_str(&d);
                self.scr.live = Some((2, tail_chars(&self.acc, 46)));
                self.scr.draw();
            }
            Ev::ThinkingDelta(t) => {
                pet_note(self.scr, pet::Ev::Thinking);
                // Wrap incrementally: only the new characters are measured.
                let wrap_cols = if self.scr.dock_columns() > 0 {
                    self.scr.transcript_width().saturating_sub(2)
                } else {
                    self.scr.cols.saturating_sub(2)
                };
                for ch in t.chars() {
                    if ch == '\n' {
                        self.think_flush();
                        continue;
                    }
                    let cw = Scr::cw(ch);
                    if !self.think_cur.is_empty() && dw(&self.think_cur) + cw > wrap_cols {
                        // Break at the last space so the reasoning panel reads
                        // as words instead of splitting mid-word at the edge.
                        if let Some(pos) = self.think_cur.rfind(' ') {
                            let tail = self.think_cur.split_off(pos + 1);
                            self.think_flush();
                            self.think_cur = tail;
                        } else {
                            self.think_flush();
                        }
                    }
                    self.think_cur.push(ch);
                }
                self.think_refresh();
                self.scr.draw();
            }
            Ev::Thinking { secs, chars } => {
                pet_note(self.scr, pet::Ev::Thinking);
                const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
                let f = FRAMES[self.spin % FRAMES.len()];
                self.spin += 1;
                let text = if chars > 0 {
                    format!("{f} {}", i18n::tf("ui_thinking", &[&format!("{secs:.1}")]))
                } else {
                    format!("{f} {}", i18n::tf("ui_working", &[&format!("{secs:.1}")]))
                };
                self.scr.live = Some((1, text));
                self.scr.draw();
            }
            Ev::ThinkingDone { secs, chars } => {
                pet_note(self.scr, pet::Ev::ThinkingDone);
                let had_lines = !self.scr.think_lines.is_empty();
                self.reset_think();
                self.scr.live = None;
                if had_lines || chars > 0 {
                    self.scr.push(
                        &format!("\u{25b8} {}", i18n::tf("ui_collapsed", &[&format!("{secs:.1}"), &chars.to_string()])),
                        K_DIM,
                    );
                }
            }
            Ev::Assistant(t) => {
                pet_note(self.scr, pet::Ev::TalkDone);
                self.reset_think();
                self.acc.clear();
                self.scr.live = None;
                // Results are the payload of the turn: render them as
                // structure (headings, bullets, code) rather than raw text.
                self.scr.push_md(&t);
            }
            Ev::ToolCall { id, name, title, args } => {
                pet_note(self.scr, pet::Ev::ToolCall(name.clone()));
                self.reset_think();
                self.scr.live = None;
                if self.scr.turn_tools.len() < 200 {
                    self.scr.turn_tools.push(name.clone());
                }
                if name == "state_report" {
                    // The pinned progress row is the whole presentation.
                    self.tools.insert(id, (name, title, std::time::Instant::now(), args));
                    return;
                }
                // Expand the call while it runs: the args are what the user
                // wants to see, not a truncated summary. Long values are
                // folded and the full text stays in CtrlO. An artifact body is
                // about to take over the whole screen, so only its title is
                // worth a card line.
                self.scr.push(&format!("* {name}"), K_TOOL);
                let cols = self.scr.cols;
                if name == "artifact" {
                    if !title.is_empty() {
                        self.scr.push(&fit(&format!("  {title}"), cols), K_DIM);
                    }
                } else {
                    for line in expand_args(&args, cols, 5) {
                        self.scr.push(&line, K_DIM);
                    }
                }
                self.tools.insert(id, (name, title, std::time::Instant::now(), args));
            }
            Ev::ToolTick { name, secs } => {
                self.scr.live = Some((1, format!("{name} running {secs:.1}s")));
                self.scr.draw();
            }
            Ev::ToolResult {
                id,
                name,
                content,
                is_error,
                ..
            } => {
                pet_note(self.scr, pet::Ev::ToolResult { name: name.clone(), is_error });
                self.scr.live = None;
                // The pending line was drawn when the call started; this one
                // adds the outcome and a short preview of what came back.
                let (name, _title, took, args) = self
                    .tools
                    .remove(&id)
                    .map(|(n, t, started, a)| (n, t, started.elapsed().as_secs_f32(), a))
                    .unwrap_or_else(|| (String::new(), String::new(), 0.0, String::new()));
                if name == "state_report" {
                    // Already shown on the pinned row; a card would duplicate it.
                    return;
                }
                // Keep the full call + result for the CtrlO viewer.
                if self.scr.tool_log.len() >= 20 {
                    self.scr.tool_log.remove(0);
                }
                let log_content: String = content.chars().take(6000).collect();
                self.scr.tool_log.push(ToolLogEntry {
                    name: name.clone(),
                    args,
                    content: log_content,
                    is_error,
                    secs: took,
                });
                let cols = self.scr.cols;
                // When sibling tools are still running in parallel, tag the
                // settled line with the tool name so out-of-order completion
                // stays readable on the 53-col panel.
                let parallel = !self.tools.is_empty();
                let verdict = if is_error { "x" } else { "ok" };
                let settled_line = if parallel && !name.is_empty() {
                    format!("  \u{2514} {name} {took:.1}s {verdict}")
                } else {
                    format!("  \u{2514} {took:.1}s {verdict}")
                };
                self.scr.push(&settled_line, K_TOOL);
                let preview = first_lines(&content, if is_error { 3 } else { 1 });
                if !preview.trim().is_empty() {
                    let kind = if is_error { K_ERR } else { K_RES };
                    // One row per card: flatten and ellipsise. The full text
                    // stays available in CtrlO, and a hard cut looked broken.
                    let flat = preview.split_whitespace().collect::<Vec<_>>().join(" ");
                    let line = format!("  \u{21b3} {flat}");
                    self.scr.push(&fit_dots(&line, cols), kind);
                }
            }
            Ev::Artifact(v) => {
                self.acc.clear();
                self.scr.live = None;
                let rows = self.scr.artifact_body_rows();
                let cols = self.scr.cols;
                if let Ok(res) = Artifact::from_tool_json(&v, rows, cols) {
                    self.scr.artifact = Some(res.artifact);
                    self.scr.draw();
                }
            }
            Ev::Context(c) => self.scr.push(&context_marker(&c), K_DIM),
            Ev::Compacting { tokens, messages } => {
                pet_note(self.scr, pet::Ev::Compact);
                self.reset_think();
                self.scr.live = Some((
                    1,
                    format!(
                        "compacting {messages} messages ({} tokens)",
                        crate::compact::human_tokens(tokens)
                    ),
                ));
                self.scr.draw();
            }
            Ev::CompactingDone { ok, note } => {
                pet_note(self.scr, pet::Ev::CompactDone { ok });
                self.scr.live = None;
                if !ok {
                    self.scr.push(&i18n::tf("ui_compaction_failed", &[&note]), K_DIM);
                }
            }
            Ev::StateReport(t) => self.scr.set_state(t),
            Ev::TurnEnd { .. } => {
                pet_note(self.scr, pet::Ev::TurnEnd);
                self.scr.flush_state();
                self.scr.draw();
            }
            Ev::Info(n) => self.scr.push(&n, K_DIM),
            Ev::Hint(n) => self.scr.push(&format!("\u{2192} {n}"), K_TOOL),
            Ev::Note(n) => self.scr.push(&n, K_ERR),
            _ => {}
        }
    }

    fn steer(&mut self) -> Option<String> {
        if self.steer.is_empty() {
            None
        } else {
            Some(self.steer.remove(0))
        }
    }

    fn confirm(&mut self, request: &crate::sandbox::Approval) -> bool {
        self.scr.approval = Some(ApprovalView {
            tool: request.tool.clone(),
            args: request.args.clone(),
            reason: request.reason.clone(),
        });
        self.scr.draw();
        let verdict = loop {
            match self.scr.read_key_timeout() {
                Some(b'y') | Some(b'Y') => break true,
                Some(b'n') | Some(b'N') | Some(0x1b) | Some(0x03) => break false,
                Some(_) => {}
                None => {}
            }
        };
        self.scr.approval = None;
        self.scr.draw();
        // read_key_timeout leaves VMIN=1; the turn's pump needs VMIN=0 again.
        poll_mode(true);
        verdict
    }

    fn ask_user(
        &mut self,
        req: &crate::ask::AskRequest,
    ) -> Result<crate::ask::AskResponse, String> {
        let title = req
            .title
            .clone()
            .unwrap_or_else(|| i18n::t("ask_title").to_string());
        let total = req.questions.len();
        let mut answers: Vec<Option<crate::ask::AskAnswer>> = vec![None; total];
        let mut qi = 0usize;
        let mut cancelled = false;

        while qi < total {
            let q = &req.questions[qi];
            let opt_labels: Vec<String> = q.options.iter().map(|o| o.label.clone()).collect();
            let mut checked = vec![false; q.options.len()];
            if let Some(prev) = &answers[qi] {
                for s in &prev.selected {
                    if let Some(idx) = q.options.iter().position(|o| o.id == s.id) {
                        checked[idx] = true;
                    }
                }
            }
            let mut view = AskView {
                title: title.clone(),
                step: qi,
                total_steps: total,
                prompt: q.prompt.clone(),
                allow_multiple: q.allow_multiple,
                options: opt_labels,
                checked,
                sel: 0,
                typing_custom: false,
                custom_buf: Vec::new(),
            };
            let mut utf8_buf: Vec<u8> = Vec::new();
            let total_rows = q.options.len() + 1;
            let other_idx = q.options.len();

            enum StepNav {
                Next(crate::ask::AskAnswer),
                Back,
                Cancel,
            }

            let nav = loop {
                self.scr.ask = Some(AskView {
                    title: view.title.clone(),
                    step: view.step,
                    total_steps: view.total_steps,
                    prompt: view.prompt.clone(),
                    allow_multiple: view.allow_multiple,
                    options: view.options.clone(),
                    checked: view.checked.clone(),
                    sel: view.sel,
                    typing_custom: view.typing_custom,
                    custom_buf: view.custom_buf.clone(),
                });
                self.scr.draw();
                let Some(ev) = self.scr.read_key_event() else {
                    continue;
                };
                if view.typing_custom {
                    match ev {
                        KeyEvent::Esc => {
                            view.typing_custom = false;
                            utf8_buf.clear();
                        }
                        KeyEvent::Char(0x03) => break StepNav::Cancel,
                        KeyEvent::Char(0x0d) | KeyEvent::Char(0x0a) => {
                            let custom: String = view.custom_buf.iter().collect();
                            let custom = custom.trim().to_string();
                            if !custom.is_empty() {
                                let selected: Vec<crate::ask::AskOption> = if q.allow_multiple {
                                    q.options
                                        .iter()
                                        .enumerate()
                                        .filter(|(i, _)| view.checked[*i])
                                        .map(|(_, o)| o.clone())
                                        .collect()
                                } else {
                                    Vec::new()
                                };
                                break StepNav::Next(crate::ask::AskAnswer {
                                    id: q.id.clone(),
                                    prompt: q.prompt.clone(),
                                    selected,
                                    custom_text: Some(custom),
                                });
                            }
                        }
                        KeyEvent::Char(0x7f) | KeyEvent::Char(0x08) => {
                            view.custom_buf.pop();
                            utf8_buf.clear();
                        }
                        KeyEvent::Char(b) if b >= 0x20 => {
                            if b < 0x80 {
                                utf8_buf.clear();
                                if view.custom_buf.len() < 200 {
                                    view.custom_buf.push(b as char);
                                }
                            } else {
                                utf8_buf.push(b);
                                let need = match utf8_buf[0] {
                                    0xf0..=0xf4 => 4,
                                    0xe0..=0xef => 3,
                                    0xc0..=0xdf => 2,
                                    _ => 1,
                                };
                                if utf8_buf.len() >= need {
                                    if let Ok(s) = std::str::from_utf8(&utf8_buf) {
                                        if let Some(ch) = s.chars().next() {
                                            if view.custom_buf.len() < 200 {
                                                view.custom_buf.push(ch);
                                            }
                                        }
                                    }
                                    utf8_buf.clear();
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                match ev {
                    KeyEvent::Esc | KeyEvent::Char(0x03) => break StepNav::Cancel,
                    KeyEvent::Up | KeyEvent::Char(b'k') => {
                        view.sel = if view.sel == 0 {
                            total_rows - 1
                        } else {
                            view.sel - 1
                        };
                    }
                    KeyEvent::Down | KeyEvent::Char(b'j') => {
                        view.sel = (view.sel + 1) % total_rows;
                    }
                    KeyEvent::PageUp => {
                        view.sel = view.sel.saturating_sub(4);
                    }
                    KeyEvent::PageDown => {
                        view.sel = (view.sel + 4).min(total_rows - 1);
                    }
                    KeyEvent::Left if qi > 0 => break StepNav::Back,
                    KeyEvent::Char(b' ') if q.allow_multiple => {
                        if view.sel < q.options.len() {
                            view.checked[view.sel] = !view.checked[view.sel];
                        } else {
                            view.typing_custom = true;
                        }
                    }
                    KeyEvent::Char(b) if (b'1'..=b'9').contains(&b) => {
                        let idx = (b - b'1') as usize;
                        if idx < q.options.len() {
                            if q.allow_multiple {
                                view.checked[idx] = !view.checked[idx];
                                view.sel = idx;
                            } else {
                                break StepNav::Next(crate::ask::AskAnswer {
                                    id: q.id.clone(),
                                    prompt: q.prompt.clone(),
                                    selected: vec![q.options[idx].clone()],
                                    custom_text: None,
                                });
                            }
                        }
                    }
                    KeyEvent::Char(0x0d) | KeyEvent::Char(0x0a) => {
                        if view.sel == other_idx {
                            view.typing_custom = true;
                        } else if q.allow_multiple {
                            let mut selected: Vec<crate::ask::AskOption> = q
                                .options
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| view.checked[*i])
                                .map(|(_, o)| o.clone())
                                .collect();
                            if selected.is_empty() {
                                selected.push(q.options[view.sel].clone());
                            }
                            break StepNav::Next(crate::ask::AskAnswer {
                                id: q.id.clone(),
                                prompt: q.prompt.clone(),
                                selected,
                                custom_text: None,
                            });
                        } else {
                            break StepNav::Next(crate::ask::AskAnswer {
                                id: q.id.clone(),
                                prompt: q.prompt.clone(),
                                selected: vec![q.options[view.sel].clone()],
                                custom_text: None,
                            });
                        }
                    }
                    _ => {}
                }
            };

            match nav {
                StepNav::Next(ans) => {
                    answers[qi] = Some(ans);
                    qi += 1;
                }
                StepNav::Back => {
                    qi = qi.saturating_sub(1);
                }
                StepNav::Cancel => {
                    cancelled = true;
                    break;
                }
            }
        }

        self.scr.ask = None;
        self.scr.draw();
        poll_mode(true);
        if cancelled {
            Ok(crate::ask::AskResponse {
                cancelled: true,
                answers: Vec::new(),
            })
        } else {
            Ok(crate::ask::AskResponse {
                cancelled: false,
                answers: answers.into_iter().flatten().collect(),
            })
        }
    }

    fn pump(&mut self) -> Action {
        if self.scr.artifact.is_some() {
            // Non-blocking drain so an open `/btw` or artifact panel never
            // stalls the background turn with 300ms blocking waits.
            self.scr.artifact_keystroke_now();
            return Action::Continue;
        }
        let mut changed = false;
        loop {
            let ev = match self.scr.read_key_event_now() {
                Some(ev) => ev,
                None => break,
            };
            match ev {
                KeyEvent::Char(0x03) | KeyEvent::Char(0x04) => {
                    crate::voice::request_stop();
                    self.scr.quit = true;
                    self.interrupted = true;
                    return Action::Cancel;
                }
                KeyEvent::Char(0x09) => {
                    // Voice interrupt: stop the turn (and any speech) right
                    // now, with the microphone already capturing, instead of
                    // waiting for the turn to finish. The take is finished once
                    // the driver unwinds: an empty prompt sends it, text already
                    // in the prompt makes it dictation.
                    self.scr.voice_start_empty = Some(self.scr.input.is_empty());
                    crate::voice::request_stop();
                    if self.scr.pending_rec.is_none() {
                        match crate::voice::start_recording() {
                            Ok(rec) => {
                                self.scr.pending_rec = Some(rec);
                                self.scr.voice_pending = true;
                            }
                            Err(e) => {
                                self.scr
                                    .push(&i18n::tf("ui_mic_failed", &[&e.to_string()]), K_ERR);
                            }
                        }
                    }
                    self.interrupted = true;
                    return Action::Cancel;
                }
                KeyEvent::Char(0x0f) => {
                    // CtrlO works during a turn: the viewer opens over the live
                    // transcript and the request keeps buffering behind it.
                    tool_output_panel(&mut self.scr);
                    poll_mode(true);
                    self.scr.draw();
                }
                KeyEvent::Char(0x11) => {
                    if !self.scr.queue.is_empty() {
                        if let Some(text) = self.scr.queue_panel_modal() {
                            self.scr.input = text.chars().collect();
                            self.scr.cursor = self.scr.input.len();
                        }
                        poll_mode(true);
                        self.scr.draw();
                    }
                }
                KeyEvent::Esc => {
                    // Esc stops the whole turn at once: the driver abandons the
                    // step instead of only cancelling the tool in flight. Chat
                    // text typed so far becomes the next message; a command is
                    // executed/refused with the same busy-time rules.
                    pet_note(self.scr, pet::Ev::Esc);
                    crate::voice::request_stop();
                    let line: String = self.scr.input.iter().collect();
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        self.scr.input.clear();
                        self.scr.cursor = 0;
                        if line.starts_with('/') {
                            self.submit_busy_line(&line);
                        } else {
                            self.force_now_line(&line);
                        }
                    }
                    self.interrupted = true;
                    return Action::Cancel;
                }
                KeyEvent::ShiftEnter => {
                    // Steering: deliver chat text at the next step boundary.
                    // A command is not chat: it takes the busy-time path.
                    let line: String = self.scr.input.iter().collect();
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        self.scr.input.clear();
                        self.scr.cursor = 0;
                        if line.starts_with('/') {
                            self.submit_busy_line(&line);
                        } else {
                            self.scr.push(
                                &format!("\u{21b3} {}", i18n::tf("ui_steer", &[&line])),
                                K_DIM,
                            );
                            self.steer.push(line);
                        }
                    }
                }
                KeyEvent::Char(0x0d) | KeyEvent::Char(0x0a) => {
                    let line: String = self.scr.input.iter().collect();
                    if !line.trim().is_empty() {
                        self.scr.input.clear();
                        self.scr.cursor = 0;
                        self.submit_busy_line(&line);
                    }
                    if self.scr.quit {
                        return Action::Cancel;
                    }
                    changed = true;
                }
                KeyEvent::Char(0x7f) | KeyEvent::Char(0x08) => {
                    if self.scr.cursor > 0 {
                        self.scr.cursor -= 1;
                        self.scr.input.remove(self.scr.cursor);
                        changed = true;
                    }
                }
                KeyEvent::Char(0x15) => {
                    self.scr.input.clear();
                    self.scr.cursor = 0;
                    changed = true;
                }
                KeyEvent::Char(0x01) => {
                    self.scr.cursor = 0;
                    changed = true;
                }
                KeyEvent::Char(0x05) => {
                    self.scr.cursor = self.scr.input.len();
                    changed = true;
                }
                KeyEvent::Char(0x17) => {
                    while self.scr.cursor > 0 && self.scr.input[self.scr.cursor - 1].is_whitespace()
                    {
                        self.scr.cursor -= 1;
                        self.scr.input.remove(self.scr.cursor);
                    }
                    while self.scr.cursor > 0 && !self.scr.input[self.scr.cursor - 1].is_whitespace()
                    {
                        self.scr.cursor -= 1;
                        self.scr.input.remove(self.scr.cursor);
                    }
                    changed = true;
                }
                KeyEvent::Left => {
                    self.scr.cursor = self.scr.cursor.saturating_sub(1);
                    changed = true;
                }
                KeyEvent::Right => {
                    self.scr.cursor = (self.scr.cursor + 1).min(self.scr.input.len());
                    changed = true;
                }
                KeyEvent::Home => {
                    self.scr.cursor = 0;
                    changed = true;
                }
                KeyEvent::End => {
                    self.scr.cursor = self.scr.input.len();
                    changed = true;
                }
                KeyEvent::Delete => {
                    if self.scr.cursor < self.scr.input.len() {
                        self.scr.input.remove(self.scr.cursor);
                        changed = true;
                    }
                }
                KeyEvent::Up => {
                    self.scr.scroll_view(true, 5);
                    changed = true;
                }
                KeyEvent::Down => {
                    self.scr.scroll_view(false, 5);
                    changed = true;
                }
                KeyEvent::PageUp => {
                    let page = self.scr.body_rows().max(1);
                    self.scr.scroll_view(true, page);
                    changed = true;
                }
                KeyEvent::PageDown => {
                    let page = self.scr.body_rows().max(1);
                    self.scr.scroll_view(false, page);
                    changed = true;
                }
                KeyEvent::Char(k) if (0x20..=0x7e).contains(&k) => {
                    if self.scr.input.len() < MAX_INPUT {
                        self.scr.input.insert(self.scr.cursor, k as char);
                        self.scr.cursor += 1;
                        changed = true;
                    }
                }
                KeyEvent::Char(k) if k >= 0x80 => {
                    self.scr.utf8_push(k);
                    changed = true;
                }
                KeyEvent::Char(_) => {}
            }
        }
        if changed {
            self.scr.draw();
        }
        Action::Continue
    }

    fn interrupted(&self) -> bool {
        self.interrupted
    }
}

impl<'a> TermUi<'a> {
    fn force_now_line(&mut self, line: &str) {
        let line = line.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.scr.queue.insert(0, line.clone());
        self.scr
            .push(&i18n::tf("force_sent", &[&line]), K_TOOL);
    }

    /// Enter while the model is working. Three classes:
    ///  * direct chat input queues (the only thing that queues);
    ///  * side-effect-free commands run right away (/usage, /btw, ...);
    ///  * commands that change the model, endpoint or history are refused with
    ///    a notice instead of being queued and replayed later.
    fn submit_busy_line(&mut self, line: &str) {
        let line = line.trim().to_string();
        if line.is_empty() {
            return;
        }
        if line.starts_with('/') {
            match cmd::busy_policy(&line) {
                cmd::BusyPolicy::Immediate => self.run_command_now(&line),
                cmd::BusyPolicy::Refuse => {
                    let shown = cmd::command_name(&line)
                        .map(|n| format!("/{n}"))
                        .unwrap_or_else(|| {
                            line.split_whitespace().next().unwrap_or("/").to_string()
                        });
                    self.scr
                        .push(&i18n::tf("ui_cmd_busy_refused", &[&shown]), K_ERR);
                    self.scr.draw();
                }
            }
            return;
        }
        if self.scr.queue.len() >= MAX_QUEUE {
            let n = self.scr.queue.len().to_string();
            self.scr.push(&i18n::tf("ui_queue_full", &[&n]), K_ERR);
        } else {
            let n = self.scr.queue.len() + 1;
            self.scr.queue.push(line.clone());
            self.scr.push(
                &format!(
                    "\u{23ce} {}",
                    i18n::tf("ui_queued", &[&n.to_string(), &line])
                ),
                K_DIM,
            );
        }
        self.scr.draw();
    }

    /// Summarize the currently running turn so a mid-turn `/btw` knows what
    /// tools or progress are active right now.
    fn in_flight_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(st) = &self.scr.state_line {
            parts.push(format!("Stage: {st}"));
        }
        if !self.tools.is_empty() {
            let mut active: Vec<String> = self
                .tools
                .values()
                .map(|t| format!("{} ({})", t.0, first_line_short(&t.1, 48)))
                .collect();
            active.sort();
            parts.push(format!("Running tools: {}", active.join(", ")));
        }
        if !self.acc.trim().is_empty() {
            parts.push(format!(
                "Partial reply so far: {}",
                first_line_short(self.acc.trim(), 120)
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    /// Run one command while the model is busy. The dispatcher is the same as
    /// on the idle path; only the outcomes that are safe mid-turn are acted on
    /// (text, /btw, /image, rename, goal, /update-hwinfo's local scan, quit).
    fn run_command_now(&mut self, line: &str) {
        self.scr.history_push(line);
        let dispatched = {
            let mut guard = self.cfg_rc.borrow_mut();
            cmd::dispatch(line, &mut guard)
        };
        let Some(out) = dispatched else {
            self.scr.push(&format!("> {line}"), K_USER);
            self.scr.draw();
            return;
        };
        // `/btw` is an ephemeral side question: never echo `> /btw ...` into
        // the main transcript.
        if out.btw.is_none() {
            self.scr.push(&format!("> {line}"), K_USER);
        }
        for (text, kind) in &out.lines {
            self.scr.push(text, *kind);
        }
        if out.image_clear {
            self.scr.attachments.clear();
            self.scr.push(i18n::t("ui_image_cleared"), K_DIM);
        }
        if let Some(arg) = out.image {
            let max = crate::vision::MAX_PER_MESSAGE;
            if self.scr.attachments.len() >= max {
                self.scr
                    .push(&i18n::tf("ui_image_too_many", &[&max.to_string()]), K_ERR);
            } else {
                match crate::vision::Attachment::load(&arg) {
                    Ok(a) => {
                        self.scr
                            .push(&i18n::tf("ui_image_added", &[&a.label()]), K_NORM);
                        self.scr.attachments.push(a);
                    }
                    Err(e) => self.scr.push(&i18n::tf("ui_image_failed", &[&e]), K_ERR),
                }
            }
        }
        if let Some(question) = out.btw {
            let question = question.trim().to_string();
            if question.is_empty() {
                self.scr.push(i18n::t("cmd_btw_usage"), K_ERR);
            } else {
                let caps = self.cfg_rc.borrow().capabilities();
                let now_cfg = api_cfg(&self.cfg_rc.borrow());
                let opts =
                    harness::RunOpts::from_config(&self.cfg_rc.borrow(), self.scr.ctx_window);
                let ctx = self.btw_ctx.clone();
                let in_flight = self.in_flight_summary();
                run_btw(
                    &mut self.scr,
                    &now_cfg,
                    caps,
                    &ctx,
                    &question,
                    &opts,
                    in_flight.as_deref(),
                );
            }
        }
        if out.model_prompt.is_some() {
            // Creator-mode /update-hwinfo wants a model turn: not while busy.
            let shown = line.split_whitespace().next().unwrap_or("/").to_string();
            self.scr
                .push(&i18n::tf("ui_cmd_busy_refused", &[&shown]), K_ERR);
        }
        if out.quit {
            crate::voice::request_stop();
            self.scr.quit = true;
            self.interrupted = true;
        }
        if let Some(arg) = out.pet {
            let mut guard = self.cfg_rc.borrow_mut();
            apply_pet(self.scr, &mut guard, &arg);
        }
        // Anything that owns the screen or rewrites history is never allowed
        // here (classification should prevent it; this is the belt-and-braces
        // path for future commands that forgot to declare themselves).
        if out.clear
            || out.login
            || out.config_wizard
            || out.resume_menu
            || out.model_menu
            || out.rewind_menu
            || out.extensions_menu
            || out.help_menu
            || out.apps_menu
            || out.compact
            || out.run_app.is_some()
        {
            let shown = line.split_whitespace().next().unwrap_or("/").to_string();
            self.scr
                .push(&i18n::tf("ui_cmd_busy_refused", &[&shown]), K_ERR);
        }
        self.scr.draw();
    }
}

/// Compact, localized one-line marker for an injected context message.
///
/// Ambient context is written for the model, not the user: the goal wrap-up
/// and repeat-call reminders are paragraphs. Dumping them into the transcript
/// pushed the actual answer off a 12-row panel, so only a marker is shown; the
/// full text still reaches the model. Human-readable injections (steering)
/// keep their first line, ellipsised.
fn context_marker(text: &str) -> String {
    let key = if text.starts_with("<goal_complete>") {
        "context_goal_complete"
    } else if text.starts_with("<goal_blocked>") {
        "context_goal_blocked"
    } else if text.starts_with("You are repeating the exact same tool call")
        || text.starts_with("Repeated tool call detected")
    {
        "context_repeat"
    } else {
        let first = text.lines().next().unwrap_or("").trim();
        return format!("\u{ff0b} {}", fit_dots(first, 50));
    };
    format!("\u{ff0b} {}", i18n::t(key))
}

fn first_line_short(s: &str, n: usize) -> String {
    let mut out: String = s.lines().next().unwrap_or("").chars().take(n).collect();
    if s.chars().count() > n {
        out.push('\u{2026}');
    }
    out
}

fn tail_chars(s: &str, n: usize) -> String {
    let v: Vec<char> = s.chars().collect();
    let start = v.len().saturating_sub(n);
    v[start..].iter().collect()
}

/// Switch the tty to non-blocking key reads for the duration of a turn.
///
/// Every loop that calls `Ui::pump` must be in this mode: the pump reads keys
/// with VMIN=0 and would otherwise block on the first iteration. Turns set it
/// from the driver loop; manual compaction and the approval pop-up restore it
/// because they read keys with their own timeout helper in between.
pub(crate) fn poll_mode(on: bool) {
    stty_tty(&["min", if on { "0" } else { "1" }, "time", "0"]);
}

// ------------------------------------------------------------------ splash

/// One animation frame, centred in the current terminal width. Rows carry their
/// splash tone (dim night sky / sea vs bright air and whale) so the scene keeps
/// its parallax on the 8-colour palette. Rows are right-padded so the block
/// always occupies the same cells even if an editor stripped trailing spaces;
/// on a terminal narrower than the panel they are truncated instead of wrapped.
fn art_frame_lines(frame: usize, cols: usize) -> Vec<(String, u8)> {
    let f = &art::FRAMES[frame % art::FRAMES.len()];
    let pad = " ".repeat(cols.saturating_sub(art::ART_WIDTH) / 2);
    f.iter()
        .enumerate()
        .map(|(row, line)| {
            let body: String = line.chars().take(cols).collect();
            let body = format!("{body:<width$}", width = cols.min(art::ART_WIDTH));
            (format!("{pad}{body}"), if art::row_is_dim(row) { K_DIM } else { K_ART })
        })
        .collect()
}

/// Identity of the terminal this process was started on: the pty slave behind
/// stdin (`pts/3`, `ttys004`, ...). `/dev/tty` is a device node rather than the
/// pty, so the fd symlink is the reliable source.
fn terminal_id() -> String {
    for link in ["/proc/self/fd/0", "/dev/fd/0"] {
        if let Ok(path) = std::fs::read_link(link) {
            let shown = path.to_string_lossy();
            let shown = shown.strip_prefix("/dev/").unwrap_or(&shown).to_string();
            if !shown.is_empty() && !shown.starts_with("fd/") {
                return shown;
            }
        }
    }
    "tty".to_string()
}

/// One lock file per terminal; slashes and odd characters become `_`.
fn tty_lock_name(tty: &str) -> String {
    let mut out: String = tty
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("tty");
    }
    out
}

/// Is `pid` a live `dst` process? Linux can name it exactly; elsewhere a live
/// pid is good enough (the lock dir is per user and stale files are reused).
fn is_dst_process(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let proc_dir = format!("/proc/{pid}");
        if !std::path::Path::new(&proc_dir).exists() {
            return false;
        }
        let comm = std::fs::read_to_string(format!("{proc_dir}/comm")).unwrap_or_default();
        matches!(comm.trim(), "dst" | "dst.exe")
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Claim one terminal for this process. Multiple dst instances are supported as
/// long as they own different terminals (the panel tty, an SSH pty, a tmux
/// pane); only a second instance on the *same* pty is refused, because two
/// readers of one `/dev/tty` make keys appear dead or land in the wrong window.
fn claim_terminal(
    lock_dir: &std::path::Path,
    tty: &str,
    pid: u32,
    alive: &dyn Fn(u32) -> bool,
) -> bool {
    let path = lock_dir.join(format!("{}.pid", tty_lock_name(tty)));
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(other) = text.trim().parse::<u32>() {
            if other != pid && alive(other) {
                return false;
            }
        }
    }
    let _ = std::fs::create_dir_all(lock_dir);
    let _ = std::fs::write(&path, pid.to_string());
    true
}

/// Claim *this* terminal (not the whole machine). The old /tmp/dst.lock/pid
/// made dst a machine-wide singleton; it is removed and only used as a fallback
/// identity when the pty cannot be resolved.
fn single_instance() -> bool {
    let _ = std::fs::remove_dir_all("/tmp/dst.lock");
    let lock_dir = crate::config::tmp_dir().join("terminals");
    claim_terminal(
        &lock_dir,
        &terminal_id(),
        std::process::id(),
        &is_dst_process,
    )
}


type ConfigRc = std::rc::Rc<std::cell::RefCell<config::Config>>;

/// Start the QR login server and render its code in the transcript.
/// The QR is 13 rows tall; inside tmux the status bar costs one row, so hide it
/// while the code is up and restore it afterwards. The pseudo-terminal also
/// grows from 12 to 13 rows, which `term_size` re-reads.
fn set_tmux_status(on: bool) {
    if std::env::var("TMUX").is_err() {
        return;
    }
    let _ = std::process::Command::new("tmux")
        .args(["set-option", "-g", "status", if on { "on" } else { "off" }])
        .status();
    std::thread::sleep(std::time::Duration::from_millis(150));
}

fn close_qr(scr: &mut Scr) {
    if scr.qr.is_none() {
        return;
    }
    set_tmux_status(true);
    let (r, c) = term_size();
    scr.rows = r;
    scr.cols = c;
    scr.qr = None;
    scr.qr_blit = false;
    {
        use std::io::Write as _;
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[2J\x1b[H");
        let _ = o.flush();
    }
    scr.draw();
}

enum LoginResult {
    Token(String),
    Key(login::Submitted),
}

/// Draw a QR login page (page URL) into the panel; returns whether the QR was
/// painted straight into the framebuffer.
fn show_page_qr(scr: &mut Scr, url: &str) -> bool {
    let (qr, note) = login::qr_lines(url);
    scr.push(&i18n::tf("login_url", &[url]), K_NORM);
    scr.push(i18n::t("login_web_started"), K_DIM);
    scr.push(i18n::t("login_web_choose"), K_DIM);
    scr.push(i18n::t("login_phone"), K_DIM);
    if qr.is_empty() {
        scr.push(&i18n::tf("login_no_qr", &[&note]), K_DIM);
        return false;
    }
    let (r, c) = term_size();
    scr.rows = r;
    scr.cols = c;
    const QR_PX: usize = 164;
    let left_cols = QR_PX / 6 + 2;
    let width = c.saturating_sub(left_cols).max(6);
    let mut guide: Vec<String> = Vec::new();
    for key in ["login_web_started", "login_web_choose", "login_phone"] {
        let text = i18n::t(key).to_string();
        let mut chunk = String::new();
        for ch in text.chars() {
            if chunk.chars().count() + 1 > width {
                guide.push(chunk.clone());
                chunk.clear();
            }
            chunk.push(ch);
        }
        if !chunk.is_empty() {
            guide.push(chunk);
        }
    }
    guide.push(String::new());
    guide.push(i18n::t("artifact_back").to_string());
    let lines: Vec<String> = guide
        .iter()
        .take(scr.rows)
        .map(|g| format!("{}{}", " ".repeat(left_cols), g))
        .collect();
    scr.qr = Some(lines);
    scr.draw();
    if login::draw_qr_fb(url, Some((0, 0, QR_PX, 170))).is_ok() {
        scr.qr_blit = true;
        true
    } else {
        false
    }
}

fn login_pick(scr: &mut Scr) -> Option<u8> {
    let items = vec![
        MenuItem { label: i18n::t("login_opt_wechat").into(), value: String::new() },
        MenuItem { label: i18n::t("login_opt_web").into(), value: String::new() },
        MenuItem { label: i18n::t("login_opt_type").into(), value: String::new() },
    ];
    let mut sel = 0usize;
    loop {
        scr.menu = Some(Menu {
            title: i18n::t("login_menu_title").into(),
            items: items.clone(),
            sel,
            hint: i18n::t("choose_hint").into(),
            preview: None,
        });
        scr.draw();
        let k = match scr.read_key_timeout() { Some(k) => k, None => continue };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => { scr.menu = None; scr.draw(); return None; }
            }
            continue;
        }
        if k == 0x03 || k == b'q' { scr.menu = None; scr.draw(); return None; }
        if k == 0x0d || k == 0x0a {
            scr.menu = None;
            scr.draw();
            return Some(sel as u8);
        }
    }
}

fn push_wrapped(guide: &mut Vec<String>, text: &str, width: usize) {
    let mut chunk = String::new();
    for ch in text.chars() {
        if chunk.chars().count() + 1 > width {
            guide.push(chunk.clone());
            chunk.clear();
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        guide.push(chunk);
    }
}

fn wechat_guide(text_cols: usize, url: &str, status: Option<String>) -> Vec<String> {
    let mut guide: Vec<String> = Vec::new();
    push_wrapped(&mut guide, i18n::t("login_wechat_title"), text_cols);
    guide.push("-".repeat(text_cols.min(23)));
    push_wrapped(&mut guide, i18n::t("login_qr_step1"), text_cols);
    push_wrapped(&mut guide, i18n::t("login_qr_step2"), text_cols);
    push_wrapped(&mut guide, i18n::t("login_qr_step3"), text_cols);
    if let Some(st) = status {
        push_wrapped(&mut guide, &format!("\u{2192} {st}"), text_cols);
    }
    guide.push(String::new());
    let page_label = i18n::tf("login_wechat_page", &[""]).trim_end().to_string();
    push_wrapped(&mut guide, &page_label, text_cols);
    push_wrapped(&mut guide, url.trim_end_matches('/'), text_cols);
    guide.push(String::new());
    push_wrapped(&mut guide, i18n::t("artifact_back"), text_cols);
    guide
}

fn login_device_status(message: &str, phase: &str, qr_ms: u64) -> Option<String> {
    // Scanned/confirming and exchanging messages win over the countdown.
    if message.to_ascii_lowercase().contains("scanned") || phase == "exchanging" {
        return login_status_text(message, phase);
    }
    if phase == "waiting" && qr_ms > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let left = 300_000u64.saturating_sub(now.saturating_sub(qr_ms)) / 1000;
        // Round to 10s so the screen only refreshes ten times a minute.
        let left = left / 10 * 10;
        let text = i18n::tf("login_status_countdown", &[&format!("{}:{:02}", left / 60, left % 60)]);
        return Some(text);
    }
    login_status_text(message, phase)
}

fn login_status_text(message: &str, phase: &str) -> Option<String> {
    if phase == "exchanging" {
        Some(i18n::t("login_status_finishing").to_string())
    } else if message.to_ascii_lowercase().contains("scanned") {
        Some(i18n::t("login_status_scanned").to_string())
    } else {
        None
    }
}

fn login_wechat_device(scr: &mut Scr, port: u16, lang: &str) -> Result<LoginResult, String> {
    let session = login::start(port, lang)?;
    let port = session.port;
    let mut attempts = 0u8;
    'retry: loop {
        login::start_wechat(&session.shared, port);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut shown = false;
        let mut blit_ok = false;
        let mut matrix: Option<Vec<String>> = None;
        let mut last_status: Option<String> = None;
        while std::time::Instant::now() < deadline {
            let (qr_version, qr, err, token, message, phase, qr_ms) = {
                let st = session.shared.lock().unwrap();
                (
                    st.qr_version,
                    st.qr.clone(),
                    st.error.clone(),
                    st.token.clone(),
                    st.message.clone(),
                    st.phase.clone(),
                    st.qr_ms,
                )
            };
            if let Some(e) = err {
                let lower = e.to_lowercase();
                if attempts == 0 && (lower.contains("expired") || lower.contains("timed out")) {
                    attempts += 1;
                    // QR expired: refresh once automatically instead of making
                    // the user re-run /login.
                    let status = Some(i18n::t("login_status_refresh").to_string());
                    let lines = if blit_ok {
                        wechat_guide(24, &session.url, status)
                    } else {
                        wechat_guide(scr.cols, &session.url, status)
                    };
                    scr.qr_blit = false;
                    scr.qr = Some(lines);
                    scr.draw();
                    if blit_ok {
                        if let Some(m) = matrix.as_ref() {
                            let _ = login::draw_matrix_fb(m, Some((156, 0, 164, 170)));
                        }
                        scr.qr_blit = true;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    continue 'retry;
                }
                // Automatic refresh is limited to one try; after that show an
                // actionable prompt so the user can retry without re-running
                // /login (e.g. flaky network or a second expiry).
                let status = Some(i18n::t("login_status_failed").to_string());
                let lines = if blit_ok {
                    wechat_guide(24, &session.url, status)
                } else {
                    wechat_guide(scr.cols, &session.url, status)
                };
                scr.qr_blit = false;
                scr.qr = Some(lines);
                scr.draw();
                if blit_ok {
                    if let Some(m) = matrix.as_ref() {
                        let _ = login::draw_matrix_fb(m, Some((156, 0, 164, 170)));
                    }
                    scr.qr_blit = true;
                }
                loop {
                    match scr.read_key_timeout() {
                        Some(b'r') | Some(b'R') => {
                            attempts = 0;
                            continue 'retry;
                        }
                        Some(b't') | Some(b'T') => {
                            session.shared.lock().unwrap().cancelled = true;
                            close_qr(scr);
                            if let Some(r) = login_type(scr) {
                                return Ok(r);
                            }
                            // User backed out of typing: fall back to the prompt.
                            break;
                        }
                        Some(0x1b) | Some(0x03) => {
                            session.shared.lock().unwrap().cancelled = true;
                            close_qr(scr);
                            return Err(i18n::t("login_cancel").into());
                        }
                        _ => {}
                    }
                }
            }
            if let Some(t) = token {
                close_qr(scr);
                return Ok(LoginResult::Token(t));
            }
            if !shown && qr_version > 0 && !qr.is_empty() {
                let m = crate::dsh_login::qr_matrix_from_jpeg(&qr);
                matrix = m.ok();
                let (r, c) = term_size();
                scr.rows = r;
                scr.cols = c;
                // QR on the right (164px), a compact numbered guide on the left.
                const QR_PX: usize = 164;
                let text_cols = c.saturating_sub(QR_PX / 6 + 1).max(10);
                let guide = wechat_guide(text_cols, &session.url, login_device_status(&message, &phase, qr_ms));
                scr.qr = Some(guide);
                scr.draw();
                blit_ok = matrix
                    .as_ref()
                    .map(|m| login::draw_matrix_fb(m, Some((156, 0, 164, 170))).is_ok())
                    .unwrap_or(false);
                scr.qr_blit = blit_ok;
                if !blit_ok {
                    let lines = wechat_guide(c, &session.url, login_device_status(&message, &phase, qr_ms));
                    scr.qr = Some(lines);
                    scr.draw();
                }
                shown = true;
                last_status = login_device_status(&message, &phase, qr_ms);
            } else if shown {
                // Show progress on the panel while the phone is scanned/confirmed.
                let status = login_device_status(&message, &phase, qr_ms);
                if status != last_status {
                    let lines = if blit_ok {
                        wechat_guide(24, &session.url, status.clone())
                    } else {
                        wechat_guide(scr.cols, &session.url, status.clone())
                    };
                    scr.qr_blit = false;
                    scr.qr = Some(lines);
                    scr.draw();
                    if blit_ok {
                        if let Some(m) = matrix.as_ref() {
                            let _ = login::draw_matrix_fb(m, Some((156, 0, 164, 170)));
                        }
                        scr.qr_blit = true;
                    }
                    last_status = status;
                }
            }
            if let Some(k) = scr.read_key_timeout() {
                if k == 0x1b || k == 0x03 {
                    session.shared.lock().unwrap().cancelled = true;
                    close_qr(scr);
                    return Err(i18n::t("login_cancel").into());
                }
            }
        }
        if attempts == 0 {
            attempts = 1;
            continue 'retry;
        }
        close_qr(scr);
        return Err(i18n::t("onboard_timeout").into());
    }
}

fn login_web(scr: &mut Scr, port: u16, lang: &str) -> Result<LoginResult, String> {
    let session = login::start(port, lang)?;
    show_page_qr(scr, &session.url);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if std::time::Instant::now() > deadline {
            close_qr(scr);
            return Err(i18n::t("onboard_timeout").into());
        }
        if let Ok(sub) = session.rx.try_recv() {
            close_qr(scr);
            if let Some(t) = sub.token.clone() {
                return Ok(LoginResult::Token(t));
            }
            return Ok(LoginResult::Key(sub));
        }
        if let Some(token) = session.shared.lock().unwrap().token.clone() {
            close_qr(scr);
            return Ok(LoginResult::Token(token));
        }
        if let Some(k) = scr.read_key_timeout() {
            if k == 0x1b || k == 0x03 {
                close_qr(scr);
                return Err(i18n::t("login_cancel").into());
            }
        }
    }
}

fn login_type(scr: &mut Scr) -> Option<LoginResult> {
    // Direct typing offers the two credential types instead of the QR paths.
    let items = vec![
        MenuItem { label: i18n::t("login_token_prompt").trim_end_matches(':').into(), value: String::new() },
        MenuItem { label: i18n::t("onboard_key").trim_end_matches(':').into(), value: String::new() },
    ];
    let mut sel = 0usize;
    loop {
        scr.menu = Some(Menu {
            title: i18n::t("login_opt_type").into(),
            items: items.clone(),
            sel,
            hint: i18n::t("choose_hint").into(),
            preview: None,
        });
        scr.draw();
        let k = match scr.read_key_timeout() { Some(k) => k, None => continue };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => { scr.menu = None; scr.draw(); return None; }
            }
            continue;
        }
        if k == 0x03 || k == b'q' { scr.menu = None; scr.draw(); return None; }
        if k == 0x0d || k == 0x0a {
            scr.menu = None;
            scr.draw();
            break;
        }
    }
    if sel == 0 {
        let token = menu_prompt(scr, i18n::t("login_token_prompt"), true)?;
        if token.trim().is_empty() {
            return None;
        }
        Some(LoginResult::Token(token.trim().to_string()))
    } else {
        let base = menu_prompt(scr, i18n::t("onboard_base"), false).unwrap_or_default();
        let key = menu_prompt(scr, i18n::t("onboard_key"), true)?;
        let model = menu_prompt(scr, i18n::t("onboard_model"), false).unwrap_or_default();
        Some(LoginResult::Key(login::Submitted {
            key: key.trim().to_string(),
            base: if base.trim().is_empty() {
                "https://api.deepseek.com/v1".into()
            } else {
                base.trim().to_string()
            },
            model: model.trim().to_string(),
            token: None,
        }))
    }
}

/// One-line notice queued during onboarding; the first-run tips screen replaces
/// the transcript, so it is pushed again after that screen is drawn.
static ONBOARD_NOTICE: std::sync::Mutex<Option<(String, u8)>> = std::sync::Mutex::new(None);

fn queue_onboard_notice(text: String, kind: u8) {
    if let Ok(mut g) = ONBOARD_NOTICE.lock() {
        *g = Some((text, kind));
    }
}

fn flush_onboard_notice(scr: &mut Scr) {
    let note = ONBOARD_NOTICE.lock().ok().and_then(|mut g| g.take());
    if let Some((text, kind)) = note {
        scr.push(&text, kind);
    }
}

/// Credentials that worked before a login attempt. A rejected key test puts
/// them back, so `/login` can never leave the device unable to chat.
#[derive(Clone)]
struct CredSnapshot {
    base: String,
    api_key: String,
    dsh_token: String,
    model: String,
    configured: bool,
}

fn cred_snapshot(config_rc: &ConfigRc) -> CredSnapshot {
    let c = config_rc.borrow();
    CredSnapshot {
        base: c.base.clone(),
        api_key: c.api_key.clone(),
        dsh_token: c.dsh_token.clone(),
        model: c.model.clone(),
        configured: c.configured,
    }
}

fn restore_credentials(config_rc: &ConfigRc, snap: &CredSnapshot) {
    let mut c = config_rc.borrow_mut();
    c.base = snap.base.clone();
    c.api_key = snap.api_key.clone();
    c.dsh_token = snap.dsh_token.clone();
    c.model = snap.model.clone();
    c.configured = snap.configured;
    let _ = c.save();
}

/// Test that the just-saved credentials can actually answer before accepting
/// them. The user asked for this explicitly: a key that does not work is
/// rejected (and the previous credentials are restored) instead of leaving a
/// device that looks configured but cannot chat.
fn verify_saved_credentials(scr: &mut Scr, config_rc: &ConfigRc, revert: &CredSnapshot) -> bool {
    let cfg = api_cfg(&config_rc.borrow());
    scr.push(i18n::t("login_testing"), K_DIM);
    scr.draw();
    let checked: Result<Option<crate::api::KeyCheck>, String> = if !cfg.token.is_empty() {
        // DSH account token: the platform has no /models, so a tiny real
        // completion (hosted by the login platform) is the only truthful test.
        crate::dsh_login::verify_token_for(&cfg.token, &cfg.model).map(|_| None)
    } else {
        crate::api::verify_key(&cfg).map(Some)
    };
    match checked {
        Ok(None) => {
            scr.push(i18n::t("login_verified"), K_DIM);
            true
        }
        Ok(Some(k)) => {
            if k.models > 0 {
                scr.push(
                    &i18n::tf("login_verified_models", &[&k.models.to_string()]),
                    K_DIM,
                );
            } else {
                scr.push(i18n::t("login_verified"), K_DIM);
            }
            if !k.model_listed {
                // The key works but the configured model is not advertised:
                // warn instead of rejecting (many gateways alias model ids).
                scr.push(&i18n::tf("login_model_warn", &[&cfg.model, &k.sample]), K_DIM);
            }
            true
        }
        Err(e) => {
            restore_credentials(config_rc, revert);
            scr.push(&i18n::tf("login_rejected", &[&e]), K_ERR);
            false
        }
    }
}

fn apply_login_result(scr: &mut Scr, config_rc: &ConfigRc, result: LoginResult) -> bool {
    let prev = cred_snapshot(config_rc);
    let saved = {
        let mut c = config_rc.borrow_mut();
        match result {
            LoginResult::Token(token) => {
                c.dsh_token = token;
                c.api_key.clear();
                c.base = "https://api.deepseek.com/v1".into();
            }
            LoginResult::Key(sub) => {
                c.api_key = sub.key;
                c.dsh_token.clear();
                if !sub.base.is_empty() {
                    c.base = sub.base;
                }
                if !sub.model.is_empty() {
                    c.model = sub.model;
                }
            }
        }
        c.configured = true;
        c.save()
    };
    match saved {
        Err(e) => {
            scr.push(&i18n::tf("ui_save_failed", &[&e.to_string()]), K_ERR);
            false
        }
        Ok(()) => {
            if !verify_saved_credentials(scr, config_rc, &prev) {
                return false;
            }
            let home = std::env::var("HOME").unwrap_or_default();
            let full = config::config_path().display().to_string();
            let path = if !home.is_empty() && full.starts_with(&home) {
                format!("~{}", &full[home.len()..])
            } else {
                full
            };
            scr.push(&i18n::tf("login_ready", &[&path]), K_NORM);
            true
        }
    }
}

/// The `/login` and onboarding login entry point: WeChat QR on the device,
/// phone page, or direct typing. Returns true when credentials were saved.
fn start_login(scr: &mut Scr, config_rc: &ConfigRc) -> bool {
    let choice = match login_pick(scr) {
        Some(c) => c,
        None => {
            scr.push(i18n::t("login_cancel"), K_DIM);
            return false;
        }
    };
    let port = login::DEFAULT_PORT;
    let lang = config_rc.borrow().language.clone();
    let result = match choice {
        0 => login_wechat_device(scr, port, &lang),
        1 => login_web(scr, port, &lang),
        _ => match login_type(scr) {
            Some(r) => Ok(r),
            None => Err(i18n::t("login_cancel").to_string()),
        },
    };
    match result {
        Ok(r) => apply_login_result(scr, config_rc, r),
        Err(e) => {
            scr.push(&i18n::tf("login_fail", &[&e]), K_ERR);
            false
        }
    }
}

/// Final first-run stage: offer to record this device's hardware in the
/// shipped hardware-info skill, so later hardware work starts from measured
/// facts. Asked once; `/update-hwinfo` re-runs the scan on demand.
fn onboarding_hardware(scr: &mut Scr, config_rc: &ConfigRc) {
    if config_rc.borrow().hw_survey_done {
        return;
    }
    scr.push(i18n::t("hw_intro"), K_NORM);
    scr.push(i18n::t("hw_ask"), K_NORM);
    scr.push(i18n::t("hw_ask_hint"), K_DIM);
    scr.draw();
    let mut answer = false;
    loop {
        match scr.read_key_timeout() {
            Some(b'y') | Some(b'Y') | Some(0x0d) | Some(0x0a) => {
                answer = true;
                break;
            }
            Some(b'n') | Some(b'N') | Some(0x1b) | Some(0x03) => break,
            Some(_) => {}
            None => {}
        }
    }
    {
        let mut c = config_rc.borrow_mut();
        c.hw_survey_done = true;
        let _ = c.save();
    }
    if !answer {
        scr.push(i18n::t("hw_scan_skipped"), K_DIM);
        return;
    }
    scr.push(i18n::t("hw_scanning"), K_DIM);
    scr.draw();
    match hwinfo::update() {
        Ok(s) => {
            let line = i18n::tf(
                "hw_scanned",
                &[
                    &s.sections.to_string(),
                    &(s.bytes / 1024).max(1).to_string(),
                    &s.version.to_string(),
                ],
            );
            scr.push(&line, K_NORM);
            queue_onboard_notice(line, K_NORM);
        }
        Err(e) => {
            let line = i18n::tf("hw_scan_failed", &[&e]);
            scr.push(&line, K_ERR);
            queue_onboard_notice(line, K_ERR);
        }
    }
}

/// First-run sandbox choice after the endpoint is configured. Esc keeps the
/// current (default auto) mode.
fn onboarding_sandbox(scr: &mut Scr, config_rc: &ConfigRc) {
    let modes = [
        ("auto", crate::sandbox::Mode::Auto),
        ("strict", crate::sandbox::Mode::Strict),
        ("full", crate::sandbox::Mode::Full),
    ];
    let current = crate::sandbox::Mode::parse(&config_rc.borrow().sandbox_mode)
        .unwrap_or(crate::sandbox::Mode::Auto);
    let mut sel = modes.iter().position(|(_, m)| *m == current).unwrap_or(0);
    loop {
        let preview = Some(vec![match modes[sel].1 {
            crate::sandbox::Mode::Auto => i18n::t("sandbox_auto_desc").to_string(),
            crate::sandbox::Mode::Strict => i18n::t("sandbox_strict_desc").to_string(),
            crate::sandbox::Mode::Full => i18n::t("sandbox_full_desc").to_string(),
        }]);
        let items = modes
            .iter()
            .map(|(name, _)| MenuItem { label: name.to_string(), value: String::new() })
            .collect();
        scr.menu = Some(Menu {
            title: i18n::t("sandbox_pick_title").into(),
            items,
            sel,
            hint: i18n::t("sandbox_pick_hint").into(),
            preview,
        });
        scr.draw();
        let k = match scr.read_key_timeout() { Some(k) => k, None => continue };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(modes.len() - 1),
                    _ => {}
                },
                _ => { scr.menu = None; scr.draw(); return; }
            }
            continue;
        }
        if k == 0x03 || k == b'q' { scr.menu = None; scr.draw(); return; }
        if k == 0x0d || k == 0x0a {
            config_rc.borrow_mut().sandbox_mode = modes[sel].1.as_str().to_string();
            let _ = config_rc.borrow().save();
            scr.menu = None;
            scr.draw();
            return;
        }
    }
}

/// First-run guide: ask DeepSeek official (QR) or a custom endpoint.
fn onboarding(scr: &mut Scr, config_rc: &ConfigRc) {
    if config_rc.borrow().configured {
        return;
    }
    let items = vec![
        MenuItem {
            label: i18n::t("onboard_opt1").into(),
            value: String::new(),
        },
        MenuItem {
            label: i18n::t("onboard_opt2").into(),
            value: String::new(),
        },
    ];
    let mut sel = 0usize;
    loop {
        let preview = if sel == 0 {
            Some(vec![
                i18n::t("onboard_prev_official_1").to_string(),
                i18n::t("onboard_prev_official_2").to_string(),
                i18n::t("onboard_prev_sandbox").to_string(),
            ])
        } else {
            Some(vec![
                i18n::t("onboard_prev_custom_1").to_string(),
                i18n::t("onboard_prev_custom_2").to_string(),
                i18n::t("onboard_prev_sandbox").to_string(),
            ])
        };
        scr.menu = Some(Menu {
            title: i18n::t("onboard_choose").into(),
            items: items.clone(),
            sel,
            hint: i18n::t("onboard_hint").into(),
            preview,
        });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => {
                    scr.menu = None;
                    scr.draw();
                    return;
                }
            }
            continue;
        }
        if k == 0x03 || k == b'q' {
            scr.menu = None;
            scr.draw();
            return;
        }
        if k == 0x0d || k == 0x0a {
            scr.menu = None;
            scr.draw();
            break;
        }
    }
    if sel == 1 {
        scr.push(i18n::t("onboard_base"), K_DIM);
        let base = match scr.edit_allow_empty() {
            Some(l) => l.trim().to_string(),
            None => return,
        };
        scr.push(i18n::t("onboard_key"), K_DIM);
        scr.mask = true;
        let key = match scr.edit() {
            Some(l) => l.trim().to_string(),
            None => return,
        };
        scr.mask = false;
        scr.push(i18n::t("onboard_model"), K_DIM);
        let model = match scr.edit_allow_empty() {
            Some(l) => l.trim().to_string(),
            None => return,
        };
        let prev = cred_snapshot(config_rc);
        let saved = {
            let mut c = config_rc.borrow_mut();
            if !base.is_empty() {
                c.base = base;
            }
            c.api_key = key;
            c.dsh_token.clear();
            if !model.is_empty() {
                c.model = model;
            }
            c.configured = !c.api_key.is_empty();
            c.save()
        };
        match saved {
            Ok(()) => {
                // A key that cannot answer is rejected here: the user is left
                // unconfigured and can run the setup again, instead of ending
                // up with a device that silently cannot talk.
                if config_rc.borrow().configured
                    && !verify_saved_credentials(scr, config_rc, &prev)
                {
                    return;
                }
                scr.push(i18n::t("onboard_saved"), K_NORM);
                onboarding_sandbox(scr, config_rc);
                onboarding_hardware(scr, config_rc);
            }
            Err(e) => scr.push(&i18n::tf("ui_save_failed", &[&e.to_string()]), K_ERR),
        }
    } else {
        if start_login(scr, config_rc) {
            onboarding_sandbox(scr, config_rc);
            onboarding_hardware(scr, config_rc);
        } else {
            scr.push(i18n::t("onboard_cancelled"), K_DIM);
        }
    }
}


// --------------------------------------------------------------- sessions


fn update_channel(cfg: &config::Config) -> crate::update::Channel {
    crate::update::Channel::parse(&cfg.update_channel).unwrap_or(crate::update::Channel::Nightly)
}

/// One-time version banner displayed on startup until the first user action.
fn startup_version_notice(cfg: &config::Config) -> String {
    let sha = crate::update::current_sha256()
        .map(|s| crate::update::short_sha(&s).to_string())
        .unwrap_or_else(|| "dev".into());
    let channel = update_channel(cfg).as_str();
    i18n::tf(
        "ui_startup_version",
        &[env!("CARGO_PKG_VERSION"), &sha, channel],
    )
}

/// `/update [nightly|stable]`: switch the channel (saved), check now and queue
/// the yes/no offer; the download/install happens only after confirmation.
fn run_update_command(scr: &mut Scr, config_rc: &ConfigRc, arg: &str) {
    if !arg.is_empty() {
        let Some(channel) = crate::update::Channel::parse(arg) else {
            return;
        };
        {
            let mut c = config_rc.borrow_mut();
            c.update_channel = channel.as_str().to_string();
            let _ = c.save();
        }
        scr.push(
            &i18n::tf("ui_update_channel_set", &[channel.as_str()]),
            K_DIM,
        );
    }
    if !crate::update::enabled() {
        scr.push(i18n::t("ui_update_dev_build"), K_ERR);
        scr.draw();
        return;
    }
    if crate::update::asset_name().is_none() {
        scr.push(i18n::t("ui_update_unsupported"), K_ERR);
        scr.draw();
        return;
    }
    let channel = update_channel(&config_rc.borrow());
    scr.busy = true;
    scr.live = Some((1, i18n::t("ui_update_checking").to_string()));
    scr.draw();
    let result = crate::update::check(channel);
    scr.busy = false;
    scr.live = None;
    match result {
        Ok(Some(info)) => {
            // The idle loop asks yes/no on its next iteration.
            scr.update_offer = Some(info);
        }
        Ok(None) => {
            let label = match crate::update::current_sha256() {
                Some(sha) => format!(
                    "{} ({})",
                    env!("CARGO_PKG_VERSION"),
                    crate::update::short_sha(&sha)
                ),
                None => env!("CARGO_PKG_VERSION").to_string(),
            };
            scr.push(
                &i18n::tf("ui_update_up_to_date", &[&label, channel.as_str()]),
                K_NORM,
            );
        }
        Err(e) => scr.push(&i18n::tf("ui_update_failed", &[&e]), K_ERR),
    }
    scr.draw();
}

fn new_session_path() -> std::path::PathBuf {
    // Millisecond suffix + collision loop: two dst instances (different
    // terminals) or a quick /new can create a session in the same second and
    // must never overwrite each other's file.
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    let stamp = std::process::Command::new("date")
        .arg("+%Y%m%d-%H%M%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "session".into())
        });
    let dir = config::sessions_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut path = dir.join(format!("{stamp}-{ms:03}.json"));
    let mut n = 1u32;
    while path.exists() {
        path = dir.join(format!("{stamp}-{ms:03}-{n}.json"));
        n += 1;
    }
    path
}

fn save_session(msgs: &jobs::Msgs, cfg: &config::Config, path: &std::path::Path) {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let doc = {
        let m = msgs.borrow();
        json::doc_messages(
            &[
                ("title", json::J::s(&crate::session::title())),
                ("cwd", json::J::s(&cwd)),
                ("model", json::J::s(&cfg.model)),
                ("effort", json::J::s(&cfg.effort)),
            ],
            &m,
        )
    };
    // Atomic replace: a crash mid-write must not leave a truncated session.
    let tmp = path.with_extension("json.tmp-write");
    if std::fs::write(&tmp, doc).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn resume_session_path(
    scr: &mut Scr,
    msgs: &jobs::Msgs,
    path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let path = path.to_path_buf();
    let label = path
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or("session")
        .to_string();
    // A session file is a full transcript. Refuse to load a monster instead
    // of parsing and copying it several times on a 446 MB device.
    const MAX_SESSION_BYTES: u64 = 24 * 1024 * 1024;
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > MAX_SESSION_BYTES {
            scr.push(
                &i18n::tf("ui_resume_too_big", &[&(m.len() / (1024 * 1024)).to_string()]),
                K_ERR,
            );
            return None;
        }
    }
    let txt = std::fs::read_to_string(&path).ok()?;
    let mut v = json::parse(&txt)?;
    drop(txt);
    // Move the array out of the parsed doc: no clone of the whole history.
    let arr = match v.take("messages") {
        json::J::Arr(a) => a,
        _ => return None,
    };
    *msgs.borrow_mut() = arr;
    let arr = msgs.borrow();
    crate::session::set_current(path.clone());
    scr.lines.clear();
    scr.scroll = 0;
    scr.splash_done = true;
    scr.art_on = false;
    let shown = crate::session::title();
    let shown = if shown.is_empty() { label } else { shown };
    scr.push(&i18n::tf("ui_resumed", &[&shown]), K_DIM);
    for m in arr.iter().rev() {
        if m.get("role").and_then(|r| r.as_str()) == Some("assistant") {
            if let Some(t) = m.get("content").and_then(|c| c.as_str()) {
                if !t.trim().is_empty() {
                    scr.push_md(t);
                }
            }
            break;
        }
    }
    Some(path)
}

// ---------------------------------------------------------- session picker

/// Relative last-talk label: `now`, `5m`, `3h`, `2d`, `1w`, `4mo`.
fn rel_time(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "now".to_string()
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else if secs < 604_800 {
        format!("{}d", secs / 86_400)
    } else if secs < 2_419_200 {
        format!("{}w", secs / 604_800)
    } else {
        format!("{}mo", secs / 2_419_200)
    }
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn short_cwd(cwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut s = cwd.to_string();
    if !home.is_empty() && s.starts_with(&home) {
        s = format!("~{}", &s[home.len()..]);
    }
    let mut out: String = s.chars().rev().take(22).collect::<String>().chars().rev().collect();
    if s.chars().count() > 22 {
        out = format!("\u{2026}{out}");
    }
    out
}

/// Menuconfig-style `/resume`: sessions grouped with the current directory
/// first, sorted by last-talk time (newest on top), each row prefixed with a
/// relative age. Typing filters by title (search is over names only), Right/Tab
/// expands the highlighted session, Enter resumes, Esc clears the search first.
fn resume_menu(scr: &mut Scr) -> Option<std::path::PathBuf> {
    let sessions = config::list_sessions();
    if sessions.is_empty() {
        scr.push(i18n::t("resume_empty"), K_DIM);
        return None;
    }
    let here = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let now = now_epoch();
    // (path, title, cwd, mtime)
    let mut entries: Vec<(std::path::PathBuf, String, String, u64)> = sessions
        .into_iter()
        .map(|(p, stem, mtime)| {
            let (title, cwd) = crate::session::meta(&p);
            let title = if title.is_empty() {
                format!("(untitled {stem})")
            } else {
                title
            };
            (p, title, cwd, mtime)
        })
        .collect();
    // Current directory first, then newest first everywhere.
    entries.sort_by(|a, b| {
        let ac = a.2 == here;
        let bc = b.2 == here;
        bc.cmp(&ac).then(b.3.cmp(&a.3))
    });
    let mut sel = 0usize;
    let mut preview: Option<Vec<String>> = None;
    let mut query = String::new();
    loop {
        // Search names first; when nothing matches, fall back to searching the
        // session bodies (Claude-Code-style memory of past conversations).
        let mut content_hits: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut shown: Vec<&(std::path::PathBuf, String, String, u64)> = entries
            .iter()
            .filter(|e| query.is_empty() || e.1.to_lowercase().contains(&query.to_lowercase()))
            .collect();
        if shown.is_empty() && !query.is_empty() {
            let q = query.to_lowercase();
            for e in entries.iter() {
                let text = std::fs::read_to_string(&e.0).unwrap_or_default().to_lowercase();
                if text.contains(&q) {
                    content_hits.insert(e.1.clone());
                    shown.push(e);
                }
            }
        }
        if sel >= shown.len() {
            sel = shown.len().saturating_sub(1);
        }
        let items: Vec<MenuItem> = shown
            .iter()
            .map(|e| MenuItem {
                label: format!(
                    "{:>4}  {}{}",
                    rel_time(now, e.3),
                    e.1,
                    if content_hits.contains(&e.1) { " [body]" } else { "" }
                ),
                value: if e.2 == here {
                    "this folder".to_string()
                } else if e.2.is_empty() {
                    String::new()
                } else {
                    short_cwd(&e.2)
                },
            })
            .collect();
        let title = if query.is_empty() {
            i18n::t("resume_title").to_string()
        } else {
            format!("{}  \u{b7}  {}", i18n::t("resume_title"), query)
        };
        let hint = if items.is_empty() {
            i18n::t("resume_no_match").to_string()
        } else {
            i18n::t("resume_hint").to_string()
        };
        scr.menu = Some(Menu {
            title,
            items,
            sel,
            hint,
            preview: preview.clone(),
        });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => {
                        sel = sel.saturating_sub(1);
                        preview = None;
                    }
                    Some(b'B') => {
                        sel = (sel + 1).min(shown.len().saturating_sub(1));
                        preview = None;
                    }
                    Some(b'C') => {
                        if let Some(e) = shown.get(sel) {
                            preview = Some(crate::session::preview(&e.0));
                        }
                    }
                    Some(b'D') => preview = None,
                    _ => {}
                },
                _ => {
                    if query.is_empty() {
                        scr.menu = None;
                        scr.draw();
                        return None;
                    }
                    query.clear();
                    preview = None;
                }
            }
            continue;
        }
        match k {
            0x03 => {
                scr.menu = None;
                scr.draw();
                return None;
            }
            0x7f | 0x08 => {
                query.pop();
                preview = None;
            }
            b'\t' => {
                if let Some(e) = shown.get(sel) {
                    preview = Some(crate::session::preview(&e.0));
                }
            }
            0x0d | 0x0a => {
                if let Some(e) = shown.get(sel) {
                    let path = e.0.clone();
                    scr.menu = None;
                    scr.draw();
                    return Some(path);
                }
            }
            0x20..=0x7e => {
                query.push(k as char);
                preview = None;
            }
            _ => {}
        }
    }
}

/// Menuconfig-style `/model`:
/// Menuconfig-style `/model`: pick from the endpoint's advertised models with
/// arrows/Enter instead of printing a numbered list.
fn model_menu(scr: &mut Scr, config_rc: &ConfigRc) {
    let cfg = api_cfg(&config_rc.borrow());
    let models = match crate::api::list_models(&cfg) {
        Ok(m) => m,
        Err(e) => {
            scr.push(&i18n::tf("ui_model_list_unavailable", &[&e.to_string()]), K_DIM);
            if let Some(name) = menu_prompt(scr, i18n::t("config_prompt_model"), false) {
                if !name.is_empty() {
                    config_rc.borrow_mut().model = name.clone();
                    let _ = config_rc.borrow().save();
                    scr.push(&i18n::tf("ui_model_set", &[&name]), K_NORM);
                }
            }
            return;
        }
    };
    if models.is_empty() {
        scr.push(i18n::t("model_none"), K_ERR);
        return;
    }
    let items: Vec<MenuItem> = models
        .iter()
        .map(|(id, _name, efforts)| MenuItem {
            label: id.clone(),
            value: efforts.join("/"),
        })
        .collect();
    let mut sel = items
        .iter()
        .position(|it| it.label == cfg.model)
        .unwrap_or(0);
    loop {
        scr.menu = Some(Menu {
            title: i18n::t("menu_model").into(),
            items: items.clone(),
            sel,
            hint: i18n::t("menu_model_hint").into(),
            preview: None,
        });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => {
                    scr.menu = None;
                    scr.draw();
                    return;
                }
            }
            continue;
        }
        if k == 0x03 || k == b'q' {
            scr.menu = None;
            scr.draw();
            return;
        }
        if k == 0x0d || k == 0x0a {
            let id = items[sel].label.clone();
            let mut c = config_rc.borrow_mut();
            c.model = id.clone();
            let _ = c.save();
            drop(c);
            scr.menu = None;
            scr.push(&i18n::tf("ui_model_set", &[&id]), K_NORM);
            return;
        }
    }
}

/// Checkpoint picker: restore the transcript of an earlier turn. Opened by
/// `/rewind` or by double-Esc.
/// `/rewind`: pick one of your previous messages; the transcript is restored to
/// just before it and that message is put back in the prompt for editing or
/// re-sending (so a wrong turn is a one-line fix, not a restart).
/// Returns `Some(prompt)` after restoring (empty for automatic/goal rounds),
/// `None` when the picker was closed without restoring.
fn rewind_menu(scr: &mut Scr, msgs: &jobs::Msgs) -> Option<String> {
    let mut list = crate::checkpoint::list();
    if list.is_empty() {
        scr.push(i18n::t("rewind_empty"), K_DIM);
        return None;
    }
    list.truncate(20);
    let now = now_epoch();
    let mut sel = 0usize;
    loop {
        let items: Vec<MenuItem> = list
            .iter()
            .map(|c| MenuItem {
                label: format!("\u{203a} {}", c.label.chars().take(46).collect::<String>()),
                value: rel_time(now, c.created),
            })
            .collect();
        let preview = Some(crate::checkpoint::preview(&list[sel].path));
        scr.menu = Some(Menu {
            title: i18n::t("rewind_title").into(),
            items,
            sel,
            hint: i18n::t("rewind_hint").into(),
            preview,
        });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(list.len() - 1),
                    _ => {}
                },
                _ => {
                    scr.menu = None;
                    scr.draw();
                    return None;
                }
            }
            continue;
        }
        if k == 0x03 || k == b'q' {
            scr.menu = None;
            scr.draw();
            return None;
        }
        if k == 0x0d || k == 0x0a {
            let chosen = list[sel].clone();
            scr.menu = None;
            let Some((_title, messages)) = crate::checkpoint::load(&chosen.path) else {
                scr.push(i18n::t("ui_checkpoint_unreadable"), K_ERR);
                scr.draw();
                return None;
            };
            // Move the restored history in; no clone of a full transcript.
            *msgs.borrow_mut() = messages;
            scr.lines.clear();
            scr.scroll = 0;
            scr.splash_done = true;
            scr.art_on = false;
            scr.attachments.clear();
            let shown = if chosen.label.is_empty() {
                i18n::t("rewind_title").to_string()
            } else {
                chosen.label.chars().take(46).collect::<String>()
            };
            scr.push(&i18n::tf("ui_rewound", &[&shown]), K_DIM);
            let restored = msgs.borrow();
            for m in restored.iter() {
                let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let text = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
                if text.trim().is_empty() {
                    continue;
                }
                match role {
                    "user" => scr.push(&format!("> {text}"), K_USER),
                    "assistant" => scr.push_md(text),
                    _ => {}
                }
            }
            drop(restored);
            scr.draw();
            return Some(chosen.prompt);
        }
    }
}
/// Skills and MCP inspection menu.
fn extensions_menu(scr: &mut Scr, config_rc: &ConfigRc) {
    let home = std::env::var("HOME").unwrap_or_default();
    let shorten = |p: String| {
        if !home.is_empty() && p.starts_with(&home) {
            format!("~{}", &p[home.len()..])
        } else {
            p
        }
    };
    let mut sel = 0usize;
    let mut preview: Option<Vec<String>> = None;
    let mut confirm: Option<String> = None;
    let mut pending: Vec<(String, u8)> = Vec::new();
    loop {
        let skills = crate::skills::load_meta();
        let disabled = config_rc.borrow().disabled_skills.clone();
        let servers = crate::mcp::server_summary();
        let mut items: Vec<MenuItem> = Vec::new();
        let mut details: Vec<Vec<String>> = Vec::new();
        // Some(name) = skill row (x toggles, d deletes when user-owned);
        // None = MCP row (inspect only).
        let mut kinds: Vec<Option<String>> = Vec::new();
        let mut deletable: Vec<bool> = Vec::new();
        for sk in &skills {
            let off = disabled.iter().any(|d| d == &sk.name);
            let model_tag = if sk.origin == "model" {
                format!("  [{}]", i18n::t("ext_model_tag"))
            } else {
                String::new()
            };
            // A package with meta.json is also a runnable app: say so here,
            // where the user manages the library, not in a second list.
            let app_marker = crate::apps::run_marker(&sk.dir);
            let app_tag = if app_marker.is_some() {
                format!("  [{}]", i18n::t("ext_app_tag"))
            } else {
                String::new()
            };
            let label = if off {
                format!(
                    "skill  {} {}{}{}",
                    sk.name,
                    i18n::t("ext_disabled"),
                    model_tag,
                    app_tag
                )
            } else {
                format!("skill  {}{}{}", sk.name, model_tag, app_tag)
            };
            items.push(MenuItem { label, value: String::new() });
            let mut d = vec![
                format!("skill: {}", sk.name),
                format!("path: {}", shorten(sk.dir.display().to_string())),
                format!(
                    "status: {}",
                    if off { i18n::t("ext_disabled") } else { "enabled" }
                ),
            ];
            if sk.origin == "model" {
                d.push(format!("origin: {}", i18n::t("ext_model_tag")));
            }
            if let Some(m) = &app_marker {
                d.push(format!("app: {m}"));
            }
            if sk.version > 1 {
                d.push(format!("version: {}", sk.version));
            }
            if !sk.updated.is_empty() {
                d.push(format!("updated: {}", sk.updated));
            }
            d.push(String::new());
            d.push(sk.description.clone());
            details.push(d);
            kinds.push(Some(sk.name.clone()));
            deletable.push(sk.is_user());
        }
        for (server, tools) in &servers {
            items.push(MenuItem {
                label: format!("mcp    {}", server),
                value: format!("{} tools", tools.len()),
            });
            let mut d = vec![format!("MCP server: {server}"), format!("tools: {}", tools.len()), String::new()];
            d.extend(tools.iter().map(|t| format!("- {t}")));
            details.push(d);
            kinds.push(None);
            deletable.push(false);
        }
        if items.is_empty() {
            scr.push(i18n::t("ext_empty"), K_DIM);
            break;
        }
        if sel >= items.len() {
            sel = items.len() - 1;
        }
        let hint = match &confirm {
            Some(name) => i18n::tf("ext_delete_ask", &[name]),
            None => i18n::t("ext_hint").to_string(),
        };
        scr.menu = Some(Menu {
            title: i18n::t("ext_title").into(),
            items: items.clone(),
            sel,
            hint,
            preview: preview.clone(),
        });
        scr.draw();
        let k = match scr.read_key_timeout() { Some(k) => k, None => continue };
        if let Some(name) = confirm.clone() {
            confirm = None;
            if k == b'y' || k == b'Y' {
                if let Some(sk) = skills.iter().find(|s| s.name == name) {
                    match crate::skills::delete(sk) {
                        Ok(_) => {
                            pending.push((i18n::tf("ext_deleted_skill", &[&name]), K_DIM));
                            let mut c = config_rc.borrow_mut();
                            if let Some(pos) = c.disabled_skills.iter().position(|d| d == &name) {
                                c.disabled_skills.remove(pos);
                                let _ = c.save();
                            }
                        }
                        Err(e) => pending.push((
                            i18n::tf("ext_delete_failed", &[&name, &e]),
                            K_ERR,
                        )),
                    }
                }
                preview = None;
            }
            continue;
        }
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => { sel = sel.saturating_sub(1); preview = None; }
                    Some(b'B') => { sel = (sel + 1).min(items.len() - 1); preview = None; }
                    Some(b'C') => {
                        // `→` opens the full SKILL.md body in the panel viewer;
                        // Enter keeps the fast metadata preview.
                        if let Some(name) = &kinds[sel] {
                            if let Some(sk) = skills.iter().find(|x| &x.name == name) {
                                let rows = scr.artifact_body_rows();
                                let cols = scr.cols;
                                scr.menu = None;
                                scr.artifact_action = 0;
                                scr.artifact = Some(Artifact::from_markdown(
                                    format!("skill {}", sk.name),
                                    &sk.body,
                                    rows,
                                    cols,
                                ));
                                scr.draw();
                                while scr.artifact.is_some() {
                                    scr.artifact_keystroke();
                                }
                                scr.draw();
                                preview = None;
                            }
                        }
                    }
                    _ => {}
                },
                _ => break,
            }
            continue;
        }
        if k == 0x03 || k == b'q' { break; }
        if k == b'x' || k == b'X' {
            if let Some(name) = &kinds[sel] {
                let mut c = config_rc.borrow_mut();
                if let Some(pos) = c.disabled_skills.iter().position(|d| d == name) {
                    c.disabled_skills.remove(pos);
                } else {
                    c.disabled_skills.push(name.clone());
                }
                let _ = c.save();
                preview = None;
            }
            continue;
        }
        if k == b'd' || k == b'D' {
            // Delete is offered only for skill packages dst owns (user dir):
            // /etc/dst/skills stays read-only, MCP rows have no package.
            if deletable[sel] {
                if let Some(name) = &kinds[sel] {
                    confirm = Some(name.clone());
                }
            }
            continue;
        }
        if k == 0x0d || k == 0x0a {
            preview = Some(details[sel].clone());
        }
    }
    scr.menu = None;
    for (text, kind) in pending {
        scr.push(&text, kind);
    }
    scr.draw();
}

/// CtrlO: browse recent tool output and open the full text in the panel viewer.
fn tool_output_panel(scr: &mut Scr) {
    if scr.tool_log.is_empty() {
        scr.push(i18n::t("tool_log_empty"), K_DIM);
        return;
    }
    let entries: Vec<ToolLogEntry> = scr.tool_log.iter().rev().cloned().collect();
    let mut sel = 0usize;
    loop {
        let items: Vec<MenuItem> = entries
            .iter()
            .map(|e| MenuItem {
                // Menu rows cannot carry per-row colour; mark failures with
                // an ASCII "x" so they stand out from successful calls.
                label: if e.is_error {
                    format!("x {}  {:.1}s", e.name, e.secs)
                } else {
                    format!("{}  {:.1}s", e.name, e.secs)
                },
                value: if e.is_error {
                    format!("error  {}", first_line_short(&e.content, 18))
                } else {
                    first_line_short(&e.content, 24)
                },
            })
            .collect();
        scr.menu = Some(Menu {
            title: i18n::t("tool_log_title").into(),
            items,
            sel,
            hint: i18n::t("tool_log_hint").into(),
            preview: None,
        });
        scr.draw();
        let k = match scr.read_key_timeout() { Some(k) => k, None => continue };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(entries.len() - 1),
                    _ => {}
                },
                _ => { scr.menu = None; scr.draw(); return; }
            }
            continue;
        }
        if k == 0x03 || k == b'q' { scr.menu = None; scr.draw(); return; }
        if k == b's' || k == b'S' {
            let e = &entries[sel];
            let path = crate::config::tmp_dir().join(format!(
                "tool-{}-{}.txt",
                crate::session::title().chars().take(20).collect::<String>().replace(' ', "-"),
                e.name
            ));
            let saved = format!("CALL {}\n{}\n\nRESULT\n{}", e.name, e.args, e.content);
            match std::fs::write(&path, saved) {
                Ok(()) => {
                    let home = std::env::var("HOME").unwrap_or_default();
                    let mut shown = path.display().to_string();
                    if !home.is_empty() && shown.starts_with(&home) {
                        shown = format!("~{}", &shown[home.len()..]);
                    }
                    scr.push(&i18n::tf("ui_saved_tool", &[&shown]), K_DIM);
                }
                Err(e) => scr.push(&i18n::tf("ui_save_failed", &[&e.to_string()]), K_ERR),
            }
            scr.menu = None;
            scr.draw();
            return;
        }
        if k == 0x0d || k == 0x0a {
            let e = &entries[sel];
            let title = if e.is_error {
                format!("{} (error)", e.name)
            } else {
                format!("{} call", e.name)
            };
            let mut body = Vec::new();
            for line in expand_args(&e.args, scr.cols, usize::MAX) {
                body.push(line);
            }
            if !e.args.trim().is_empty() {
                body.push(String::new());
            }
            for line in e.content.lines() {
                let mut wrapped = Vec::new();
                Scr::wrap_into(&mut wrapped, line, scr.cols);
                if wrapped.is_empty() { body.push(String::new()); }
                body.extend(wrapped);
            }
            let rows = scr.artifact_body_rows();
            let cols = scr.cols;
            scr.menu = None;
            let mut a = Artifact::from_lines_plain(title, &body, rows, cols);
            if e.is_error {
                a.theme = crate::artifact::Theme::Error;
            }
            scr.artifact = Some(a);
            scr.draw();
            return;
        }
    }
}

/// `/help`: a menu of commands (name left, one-line description right).
/// Enter runs a no-argument command, or completes a command that needs input.
fn help_menu(scr: &mut Scr) -> Option<String> {
    let mut ordered: Vec<&cmd::Command> = cmd::COMMANDS.iter().collect();
    ordered.sort_by(|a, b| {
        cmd::category_rank(a.category)
            .cmp(&cmd::category_rank(b.category))
            .then(a.name.cmp(b.name))
    });
    let sorted: Vec<cmd::Command> = ordered
        .into_iter()
        .map(|c| cmd::Command {
            name: c.name,
            description: c.description,
            input_hint: c.input_hint,
            category: c.category,
        })
        .collect();
    let items: Vec<MenuItem> = sorted
        .iter()
        .map(|c| MenuItem {
            label: format!("/{}", c.name),
            value: format!("[{}] {}", i18n::t(c.category), cmd::describe(c.name)),
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    let mut sel = 0usize;
    loop {
        let c = &sorted[sel];
        let hint = c.input_hint.map(|h| format!(" {h}")).unwrap_or_default();
        // Only two footer rows: the command line and what it does. The
        // key hints below already say Enter runs (and completes) it, which
        // buys the list one more visible row on the 12-row panel.
        let preview = Some(vec![
            format!("/{}{hint}", c.name),
            cmd::describe(c.name),
        ]);
        scr.menu = Some(Menu {
            title: i18n::t("cmd_help_title").into(),
            items: items.clone(),
            sel,
            hint: i18n::t("help_hint").into(),
            preview,
        });
        scr.draw();
        let k = match scr.read_key_timeout() { Some(k) => k, None => continue };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => { scr.menu = None; scr.draw(); return None; }
            }
            continue;
        }
        if k == 0x03 || k == b'q' { scr.menu = None; scr.draw(); return None; }
        if k == 0x0d || k == 0x0a {
            let c = &sorted[sel];
            let needs_arg = c.input_hint.map(|h| h.starts_with('<')).unwrap_or(false);
            let chosen = if needs_arg {
                format!("/{} ", c.name)
            } else {
                format!("/{}", c.name)
            };
            scr.menu = None;
            scr.draw();
            return Some(chosen);
        }
    }
}

// ---------------------------------------------------------- config wizard

/// prompt inside the menu screens (menu is hidden while typing)
fn menu_prompt(scr: &mut Scr, prompt: &str, mask: bool) -> Option<String> {
    scr.menu = None;
    scr.mask = mask;
    scr.push(prompt, K_DIM);
    scr.draw();
    let r = scr.edit();
    scr.mask = false;
    r.map(|s| s.trim().to_string())
}

/// menuconfig-style configuration: arrows move, Enter edits, Esc leaves.
fn config_menu(scr: &mut Scr, config_rc: &ConfigRc) {
    let mut sel = 0usize;
    loop {
        let c = config_rc.borrow();
        let (model, effort, base, kstate, voice, auto, queued, lang, mode, steps, compact, goalr, sandbox_mode, judge_model, caps) = (
            c.model.clone(),
            c.effort.clone(),
            c.base.clone(),
            if !c.dsh_token.is_empty() {
                i18n::t("dsh_token_set").to_string()
            } else if c.api_key.is_empty() {
                i18n::t("unset").to_string()
            } else {
                i18n::t("set").to_string()
            },
            c.tts_voice.clone(),
            c.auto_send_voice,
            c.queued_default,
            c.language.clone(),
            if c.creator { "creator".to_string() } else { "normal".to_string() },
            if c.max_steps == 0 { i18n::t("unlimited").to_string() } else { c.max_steps.to_string() },
            if c.compact_enabled {
                format!("{} @ {}", i18n::t("on"), crate::compact::human_tokens(c.compact_threshold))
            } else {
                i18n::t("off").to_string()
            },
            if c.goal_max_rounds == 0 {
                i18n::t("unlimited").to_string()
            } else {
                c.goal_max_rounds.to_string()
            },
            c.sandbox_mode.clone(),
            if c.sandbox_judge_model.is_empty() {
                i18n::t("same_as_model").to_string()
            } else {
                c.sandbox_judge_model.clone()
            },
            c.capabilities(),
        );
        drop(c);
        let items = vec![
            MenuItem { label: i18n::t("menu_model").into(), value: model },
            MenuItem { label: i18n::t("menu_effort").into(), value: effort },
            MenuItem { label: i18n::t("menu_endpoint").into(), value: base },
            MenuItem { label: i18n::t("menu_key").into(), value: kstate },
            MenuItem { label: i18n::t("menu_voice").into(), value: voice },
            MenuItem {
                label: i18n::t("menu_autosend").into(),
                value: if auto { i18n::t("on").into() } else { i18n::t("off").into() },
            },
            MenuItem {
                label: i18n::t("menu_queue").into(),
                value: if queued { i18n::t("queued_mode").into() } else { i18n::t("immediate_mode").into() },
            },
            MenuItem { label: i18n::t("menu_language").into(), value: lang },
            MenuItem { label: i18n::t("menu_mode").into(), value: mode },
            MenuItem { label: i18n::t("menu_steps").into(), value: steps },
            MenuItem { label: i18n::t("menu_compact").into(), value: compact },
            MenuItem { label: i18n::t("menu_goalrounds").into(), value: goalr },
            MenuItem { label: i18n::t("menu_sandbox").into(), value: sandbox_mode },
            MenuItem { label: i18n::t("menu_sandbox_judge").into(), value: judge_model },
            MenuItem { label: i18n::t("menu_extensions").into(), value: String::new() },
            MenuItem { label: i18n::t("menu_save").into(), value: String::new() },
        ];
        let hint = match (caps.web_tools, caps.speech) {
            // Speech is only mocked open in the simulator; keep the banner
            // honest about what this endpoint really provides.
            (true, true) => i18n::t("menu_hint"),
            (false, true) => i18n::t("menu_hint_no_web"),
            _ => i18n::t("menu_hint_gated"),
        }
        .to_string();
        let help_keys = [
            "cfg_help_model",
            "cfg_help_effort",
            "cfg_help_endpoint",
            "cfg_help_key",
            "cfg_help_voice",
            "cfg_help_autosend",
            "cfg_help_queue",
            "cfg_help_language",
            "cfg_help_mode",
            "cfg_help_steps",
            "cfg_help_compact",
            "cfg_help_goalrounds",
            "cfg_help_sandbox",
            "cfg_help_sandbox_judge",
            "cfg_help_extensions",
            "cfg_help_save",
        ];
        let preview = Some(vec![
            i18n::t(help_keys[sel.min(help_keys.len() - 1)]).to_string(),
        ]);
        scr.menu = Some(Menu { title: i18n::t("menu_title").into(), items: items.clone(), sel, hint, preview });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => sel = (sel + 1).min(items.len() - 1),
                    _ => {}
                },
                _ => {
                    scr.menu = None;
                    scr.draw();
                    return;
                }
            }
            continue;
        }
        if k == 0x03 || k == b'q' {
            scr.menu = None;
            scr.draw();
            return;
        }
        if k != 0x0d {
            continue;
        }
        match sel {
            8 => {
                let mut c = config_rc.borrow_mut();
                c.creator = !c.creator;
                let _ = c.save();
            }
            0 => {
                let cfg = api_cfg(&config_rc.borrow());
                let models = crate::api::list_models(&cfg).unwrap_or_default();
                if models.is_empty() {
                    if let Some(v) = menu_prompt(scr, i18n::t("config_prompt_model"), false) {
                        if !v.is_empty() {
                            config_rc.borrow_mut().model = v;
                            let _ = config_rc.borrow().save();
                        }
                    }
                } else {
                    let mut msel = 0usize;
                    loop {
                        let its: Vec<MenuItem> = models
                            .iter()
                            .map(|(id, name, _)| MenuItem { label: id.clone(), value: fit(name, 18) })
                            .collect();
                        scr.menu = Some(Menu {
                            title: i18n::t("menu_model").into(),
                            items: its,
                            sel: msel,
                            hint: i18n::t("menu_model_hint").into(),
                            preview: None,
                        });
                        scr.draw();
                        let kk = match scr.read_key_timeout() { Some(k) => k, None => continue };
                        if kk == 0x1b {
                            match scr.read_key_after_esc() {
                                Some(b'[') => {
                                    match scr.read_key_timeout() {
                                        Some(b'A') => msel = msel.saturating_sub(1),
                                        Some(b'B') => msel = (msel + 1).min(models.len() - 1),
                                        _ => {}
                                    }
                                }
                                _ => break,
                            }
                            continue;
                        }
                        if kk == 0x0d {
                            config_rc.borrow_mut().model = models[msel].0.clone();
                            let _ = config_rc.borrow().save();
                            break;
                        }
                    }
                }
            }
            1 => {
                let cur = config_rc.borrow().effort.clone();
                let next = match cur.as_str() {
                    "auto" => "low",
                    "low" => "high",
                    "high" => "max",
                    _ => "auto",
                };
                config_rc.borrow_mut().effort = next.to_string();
                let _ = config_rc.borrow().save();
            }
            2 => {
                let mut esel = if crate::config::is_official(&config_rc.borrow().base) { 0 } else { 1 };
                loop {
                    let its = vec![
                        MenuItem { label: i18n::t("menu_official").into(), value: "api.deepseek.com".into() },
                        MenuItem { label: i18n::t("menu_custom").into(), value: config_rc.borrow().base.clone() },
                    ];
                    scr.menu = Some(Menu { title: i18n::t("menu_endpoint").into(), items: its, sel: esel, hint: i18n::t("menu_endpoint_hint").into(), preview: None });
                    scr.draw();
                    let kk = match scr.read_key_timeout() { Some(k) => k, None => continue };
                    if kk == 0x1b {
                        match scr.read_key_after_esc() {
                            Some(b'[') => {
                                match scr.read_key_timeout() {
                                    Some(b'A') => esel = 0,
                                    Some(b'B') => esel = 1,
                                    _ => {}
                                }
                            }
                            _ => break,
                        }
                        continue;
                    }
                    if kk != 0x0d {
                        continue;
                    }
                    if esel == 0 {
                        config_rc.borrow_mut().base = "https://api.deepseek.com/v1".into();
                        let _ = config_rc.borrow().save();
                        break;
                    }
                    // Warn before a third-party endpoint: capabilities shrink.
                    scr.menu = None;
                    scr.draw();
                    scr.push(i18n::t("menu_gate_warn"), K_ERR);
                    scr.push(i18n::t("menu_gate_ask"), K_DIM);
                    let go = match scr.edit() { Some(a) => a.trim().eq_ignore_ascii_case("y"), None => false };
                    if go {
                        if let Some(u) = menu_prompt(scr, "Base URL (https://host/v1):", false) {
                            if u.starts_with("http") {
                                config_rc.borrow_mut().base = u;
                                let _ = config_rc.borrow().save();
                            } else if !u.is_empty() {
                                scr.push(i18n::t("ui_http_only"), K_ERR);
                            }
                        }
                    }
                    break;
                }
            }
            3 => {
                if let Some(k) = menu_prompt(scr, i18n::t("onboard_key"), true) {
                    if !k.is_empty() {
                        let mut c = config_rc.borrow_mut();
                        c.api_key = k;
                        c.dsh_token.clear();
                        c.configured = true;
                        let _ = c.save();
                    }
                }
            }
            4 => {
                let cur = config_rc.borrow().tts_voice.clone();
                config_rc.borrow_mut().tts_voice = if cur == "stella" { "tide".into() } else { "stella".into() };
                let _ = config_rc.borrow().save();
            }
            5 => {
                let mut c = config_rc.borrow_mut();
                c.auto_send_voice = !c.auto_send_voice;
                let _ = c.save();
            }
            6 => {
                let mut c = config_rc.borrow_mut();
                c.queued_default = !c.queued_default;
                let _ = c.save();
            }
            7 => {
                let cur = config_rc.borrow().language.clone();
                let idx = i18n::CODES
                    .iter()
                    .position(|c| cur.to_ascii_lowercase().starts_with(c))
                    .unwrap_or(0);
                let next = i18n::CODES[(idx + 1) % i18n::CODES.len()];
                config_rc.borrow_mut().language = next.to_string();
                i18n::set(next);
                let _ = config_rc.borrow().save();
            }
            9 => {
                if let Some(v) = menu_prompt(scr, i18n::t("config_prompt_steps"), false) {
                    match v.parse::<u32>() {
                        Ok(n) => {
                            config_rc.borrow_mut().max_steps = n;
                            let _ = config_rc.borrow().save();
                        }
                        Err(_) => scr.push(i18n::t("config_expected_number"), K_ERR),
                    }
                }
            }
            10 => {
                if let Some(v) = menu_prompt(
                    scr,
                    i18n::t("config_prompt_compact"),
                    false,
                ) {
                    match v.parse::<u64>() {
                        Ok(n) => {
                            let mut c = config_rc.borrow_mut();
                            c.compact_threshold = n;
                            c.compact_enabled = n > 0;
                            let _ = c.save();
                        }
                        Err(_) => scr.push(i18n::t("config_expected_tokens"), K_ERR),
                    }
                }
            }
            11 => {
                if let Some(v) = menu_prompt(scr, i18n::t("config_prompt_goal"), false) {
                    match v.parse::<u32>() {
                        Ok(n) => {
                            config_rc.borrow_mut().goal_max_rounds = n;
                            let _ = config_rc.borrow().save();
                        }
                        Err(_) => scr.push(i18n::t("config_expected_number"), K_ERR),
                    }
                }
            }
            13 => {
                if let Some(v) = menu_prompt(
                    scr,
                    i18n::t("config_prompt_judge"),
                    false,
                ) {
                    let mut c = config_rc.borrow_mut();
                    c.sandbox_judge_model = v.trim().to_string();
                    let _ = c.save();
                }
            }
            14 => {
                scr.menu = None;
                scr.draw();
                extensions_menu(scr, config_rc);
            }
            12 => {
                let mut c = config_rc.borrow_mut();
                c.sandbox_mode = crate::sandbox::Mode::parse(&c.sandbox_mode)
                    .unwrap_or(crate::sandbox::Mode::Auto)
                    .next()
                    .as_str()
                    .to_string();
                let _ = c.save();
            }
            _ => {
                let _ = config_rc.borrow().save();
                scr.menu = None;
                scr.draw();
                return;
            }
        }
    }
}



/// Saved-app manager (creator mode): list, preview the annotation, run, delete.
/// Preview a prompt app's request in the panel viewer. Returns true when the
/// user pressed Enter to send it; Esc (or q) cancels back to the menu.
fn review_prompt_app(scr: &mut Scr, app: &crate::apps::App) -> bool {
    let text = crate::apps::prompt_body(app);
    let mut body: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut wrapped = Vec::new();
        Scr::wrap_into(&mut wrapped, line, scr.cols);
        if wrapped.is_empty() {
            body.push(String::new());
        } else {
            body.extend(wrapped);
        }
    }
    if body.is_empty() {
        body.push(String::new());
    }
    let rows = scr.artifact_body_rows();
    let cols = scr.cols;
    scr.menu = None;
    scr.artifact_action = 0;
    let mut a = Artifact::from_lines_plain(app.title.clone(), &body, rows, cols);
    a.submit = true;
    scr.artifact = Some(a);
    scr.draw();
    while scr.artifact.is_some() {
        scr.artifact_keystroke();
    }
    scr.draw();
    scr.artifact_action == 1
}

/// What the /apps launcher wants done after it closes.
enum AppsAction {
    None,
    /// Submit this prompt app's request as the next conversation turn.
    RunPrompt(String, String),
    /// Pre-fill the prompt so the user can describe an app to build.
    NewApp,
    /// Pre-fill the prompt asking the model to change this app.
    Improve(String),
}

/// Creator app launcher: run/read/delete apps, replay captured output, search,
/// and (in creator mode) a "+ new app" row that seeds the prompt.
fn apps_menu(scr: &mut Scr, config_rc: &ConfigRc) -> AppsAction {
    let home = std::env::var("HOME").unwrap_or_default();
    let shorten = |p: String| {
        if !home.is_empty() && p.starts_with(&home) {
            format!("~{}", &p[home.len()..])
        } else {
            p
        }
    };
    // Skill panel openable from the list (`->` or doc-app Enter): the
    // meta.json facts first, then the SKILL.md instructions.
    fn panel_body(app: &crate::apps::App, short_path: String) -> Vec<String> {
        let mut d = vec![
            i18n::tf("apps_path", &[&short_path]),
            i18n::tf("apps_kind", &[&app.kind]),
        ];
        if !app.entry.is_empty() {
            d.push(i18n::tf("apps_entry", &[&app.entry]));
        }
        d.push(i18n::tf("apps_version", &[&app.version.to_string()]));
        d.push(i18n::tf("apps_updated", &[&app.updated]));
        if !app.default_args.is_empty() {
            d.push(i18n::tf("apps_args", &[&app.default_args]));
        }
        if !app.requires.is_empty() {
            d.push(i18n::tf("apps_requires", &[&app.requires.join(", ")]));
        }
        for (label, stamp) in [
            ("apps_last_run", &app.last_run),
            ("apps_last_test", &app.last_test),
        ] {
            if let Some(r) = stamp {
                d.push(i18n::tf(
                    label,
                    &[&r.outcome, &crate::apps::stamp_hhmm(&r.at)],
                ));
            }
        }
        let files = crate::apps::files(app);
        if !files.is_empty() {
            let joined = files
                .iter()
                .take(6)
                .map(|(n, b)| format!("{n} ({b}B)"))
                .collect::<Vec<_>>()
                .join(", ");
            d.push(i18n::tf("apps_files", &[&joined]));
        }
        d.push(String::new());
        let body = crate::apps::annotation(app);
        let mut lines: Vec<String> = body
            .lines()
            .map(|l| l.trim_end())
            .filter(|l| {
                let t = l.trim_start();
                !t.is_empty()
                    && !t.starts_with("# ")
                    && !t.starts_with("**Kind:**")
                    && !t.starts_with("**Entry:**")
                    && !t.starts_with("**Path:**")
            })
            .map(|l| l.to_string())
            .collect();
        if lines.is_empty() {
            lines.push(app.description.clone());
        }
        d.extend(lines);
        d
    }
    fn open_panel(scr: &mut Scr, app: &crate::apps::App, short_path: String) {
        scr.menu = None;
        scr.draw();
        let rows = scr.artifact_body_rows();
        let cols = scr.cols;
        let text = panel_body(app, short_path).join("\n");
        scr.artifact = Some(Artifact::from_markdown(app.title.clone(), &text, rows, cols));
        scr.draw();
        while scr.artifact.is_some() {
            scr.artifact_keystroke();
        }
        scr.draw();
    }
    #[derive(Clone, Copy)]
    enum Row {
        App(usize),
        New,
    }
    let mut sel = 0usize;
    let mut confirm: Option<String> = None;
    let mut pending: Vec<(String, u8)> = Vec::new();
    let mut searching = false;
    let mut filter = String::new();
    let mut filter_before_search = String::new();
    let mut utf8: Vec<u8> = Vec::new();
    loop {
        let creator = config_rc.borrow().creator;
        let apps = crate::apps::load();
        if !creator && apps.is_empty() {
            scr.menu = None;
            pending.push((i18n::t("cmd_apps_empty").to_string(), K_DIM));
            break;
        }
        let needle = filter.trim().to_lowercase();
        let mut filtered: Vec<&crate::apps::App> = apps
            .iter()
            .filter(|a| {
                needle.is_empty()
                    || a.name.to_lowercase().contains(&needle)
                    || a.title.to_lowercase().contains(&needle)
                    || a.description.to_lowercase().contains(&needle)
            })
            .collect();
        // Launcher order: most recently run/tested first, then alphabetical.
        filtered.sort_by(|a, b| {
            let la = crate::apps::last_activity(a);
            let lb = crate::apps::last_activity(b);
            lb.cmp(&la).then_with(|| a.name.cmp(&b.name))
        });
        let mut rows: Vec<Row> = (0..filtered.len()).map(Row::App).collect();
        if creator {
            rows.push(Row::New);
        }
        if sel >= rows.len() {
            sel = rows.len().saturating_sub(1);
        }
        let items: Vec<MenuItem> = rows
            .iter()
            .map(|row| match row {
                Row::App(i) => {
                    let a = filtered[*i];
                    let bad = crate::apps::is_broken(a);
                    let used = crate::apps::last_used_hhmm(a)
                        .map(|t| format!(" · {t}"))
                        .unwrap_or_default();
                    MenuItem {
                        label: if a.title.is_empty() { a.name.clone() } else { a.title.clone() },
                        value: format!("{}{}{}", a.kind, if bad { " !" } else { "" }, used),
                    }
                }
                Row::New => MenuItem {
                    label: i18n::t("apps_new_row").to_string(),
                    value: String::new(),
                },
            })
            .collect();
        let hint = if let Some(name) = &confirm {
            i18n::tf("apps_delete_ask", &[name])
        } else if searching {
            i18n::t("apps_search_hint").to_string()
        } else if !filter.is_empty() {
            i18n::tf("apps_filter", &[&filter])
        } else {
            i18n::t("apps_hint").to_string()
        };
        let title = if creator {
            let skills = crate::skills::load_meta().len().to_string();
            let skills = i18n::tf("apps_skills_count", &[&skills]);
            let broken = apps.iter().filter(|a| crate::apps::is_broken(a)).count();
            let count = if filter.is_empty() {
                apps.len().to_string()
            } else {
                format!("{}/{}", filtered.len(), apps.len())
            };
            if broken > 0 {
                format!(
                    "{} · {} · {}! · {}",
                    i18n::t("apps_title"),
                    count,
                    broken,
                    skills
                )
            } else {
                format!("{} · {} · {}", i18n::t("apps_title"), count, skills)
            }
        } else if filter.is_empty() {
            format!("{} · {}", i18n::t("apps_title"), apps.len())
        } else {
            format!("{} · {}/{}", i18n::t("apps_title"), filtered.len(), apps.len())
        };
        let preview = if filtered.is_empty() && !needle.is_empty() && !confirm.is_some() {
            Some(vec![i18n::t("apps_no_match").to_string()])
        } else {
            None
        };
        scr.menu = Some(Menu {
            title,
            items: items.clone(),
            sel,
            hint,
            preview,
        });
        scr.draw();
        let k = match scr.read_key_timeout() {
            Some(k) => k,
            None => continue,
        };
        if let Some(name) = confirm.clone() {
            confirm = None;
            if k == b'y' || k == b'Y' {
                match apps.iter().find(|a| a.label() == name) {
                    Some(app) => match crate::apps::delete(app) {
                        Ok(()) => pending.push((i18n::tf("apps_deleted", &[&name]), K_DIM)),
                        Err(e) => pending.push((
                            i18n::tf("apps_delete_failed", &[&name, &e]),
                            K_ERR,
                        )),
                    },
                    None => {}
                }
            }
            continue;
        }
        if searching {
            match k {
                0x1b => {
                    searching = false;
                    filter = filter_before_search.clone();
                    utf8.clear();
                    sel = 0;
                }
                0x0d | 0x0a => searching = false,
                0x7f | 0x08 => {
                    filter.pop();
                    sel = 0;
                }
                0x03 => {
                    searching = false;
                    filter = filter_before_search.clone();
                    utf8.clear();
                    sel = 0;
                }
                0x20..=0x7e => {
                    filter.push(k as char);
                    sel = 0;
                }
                0x80..=0xff => {
                    utf8.push(k);
                    if let Ok(text) = std::str::from_utf8(&utf8) {
                        filter.push_str(text);
                        utf8.clear();
                        sel = 0;
                    }
                    if utf8.len() > 4 {
                        utf8.clear();
                    }
                }
                _ => {}
            }
            continue;
        }
        if k == 0x1b {
            match scr.read_key_after_esc() {
                Some(b'[') => match scr.read_key_timeout() {
                    Some(b'A') => sel = sel.saturating_sub(1),
                    Some(b'B') => {
                        if !rows.is_empty() {
                            sel = (sel + 1).min(rows.len() - 1);
                        }
                    }
                    Some(b'C') => {
                        if let Row::App(i) = rows[sel] {
                            let app = filtered[i];
                            open_panel(scr, app, shorten(app.path()));
                        }
                    }
                    _ => {}
                },
                _ => {
                    if !filter.is_empty() {
                        filter.clear();
                        sel = 0;
                        continue;
                    }
                    scr.menu = None;
                    break;
                }
            }
            continue;
        }
        if k == 0x03 || k == b'q' {
            scr.menu = None;
            break;
        }
        if k == b'/' {
            searching = true;
            filter_before_search = filter.clone();
            utf8.clear();
            sel = 0;
            continue;
        }
        if rows.is_empty() {
            continue;
        }
        match rows[sel] {
            Row::New => {
                if k == b'\r' || k == b'\n' {
                    scr.menu = None;
                    for (text, kind) in pending.drain(..) {
                        scr.push(&text, kind);
                    }
                    scr.draw();
                    return AppsAction::NewApp;
                }
                continue;
            }
            Row::App(i) => {
                let app = filtered[i].clone();
                if k == b'x' || k == b'X' {
                    confirm = Some(app.label());
                    continue;
                }
                if (k == b'i' || k == b'I') && creator {
                    scr.menu = None;
                    for (text, kind) in pending.drain(..) {
                        scr.push(&text, kind);
                    }
                    scr.draw();
                    return AppsAction::Improve(app.label());
                }
                if k == b'l' || k == b'L' {
                    let log_path = app.dir.join("state").join("last-run.log");
                    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
                    let mut body: Vec<String> = Vec::new();
                    if text.trim().is_empty() {
                        Scr::wrap_into(&mut body, i18n::t("apps_no_log"), scr.cols);
                    } else {
                        if let Some(r) = &app.last_run {
                            body.push(i18n::tf(
                                "apps_last_run",
                                &[&r.outcome, &crate::apps::stamp_hhmm(&r.at)],
                            ));
                        }
                        body.push(String::new());
                        for line in text.lines() {
                            let mut wrapped = Vec::new();
                            Scr::wrap_into(&mut wrapped, line, scr.cols);
                            if wrapped.is_empty() {
                                body.push(String::new());
                            } else {
                                body.extend(wrapped);
                            }
                        }
                    }
                    let rows = scr.artifact_body_rows();
                    let cols = scr.cols;
                    scr.menu = None;
                    scr.artifact_action = 0;
                    scr.artifact = Some(Artifact::from_lines_plain(
                        format!("{} · {}", app.title, i18n::t("apps_log")),
                        &body,
                        rows,
                        cols,
                    ));
                    scr.draw();
                    while scr.artifact.is_some() {
                        scr.artifact_keystroke();
                    }
                    scr.draw();
                    continue;
                }
                if k == b'\r' || k == b'\n' {
                    if app.kind == "doc" {
                        open_panel(scr, &app, shorten(app.path()));
                        continue;
                    }
                    if app.kind == "prompt" {
                        if review_prompt_app(scr, &app) {
                            for (text, kind) in pending.drain(..) {
                                scr.push(&text, kind);
                            }
                            scr.draw();
                            return AppsAction::RunPrompt(
                                app.label(),
                                crate::apps::prompt_body(&app),
                            );
                        }
                        continue;
                    }
                    scr.menu = None;
                    scr.draw();
                    match crate::apps::run_on_screen_args(
                        &app,
                        crate::apps::DIRECT_TIMEOUT_S,
                        &app.default_args,
                    ) {
                        Ok(_) => pending.push((i18n::tf("apps_exited", &[&app.label()]), K_DIM)),
                        Err(e) => pending.push((e, K_ERR)),
                    }
                    continue;
                }
            }
        }
    }
    scr.menu = None;
    for (text, kind) in pending {
        scr.push(&text, kind);
    }
    scr.draw();
    AppsAction::None
}

/// One-time, actionable key tips after the first successful configuration.
fn maybe_show_tips(scr: &mut Scr, config_rc: &ConfigRc) {
    let configured = config_rc.borrow().configured;
    let shown = config_rc.borrow().tips_shown;
    if !configured || shown {
        return;
    }
    // Tips are the first real screen: drop the splash art so they start at
    // row 0, but keep `splash_done` false so the very first prompt clears
    // them again instead of leaving stale onboarding text in the transcript.
    scr.lines.clear();
    scr.scroll = 0;
    scr.art_on = false;
    scr.push(i18n::t("tagline"), K_HEAD);
    for key in ["tip1", "tip2", "tip3"] {
        scr.push(&format!("- {}", i18n::t(key)), K_DIM);
    }
    scr.push(&format!("  {}", i18n::t("tip_more")), K_DIM);
    let mut c = config_rc.borrow_mut();
    c.tips_shown = true;
    let _ = c.save();
}


/// Effective tool set: capability-filtered base registry, feature tools,
/// creator tools while creator mode is on, minus `disabled_tools`, deduplicated
/// across the whole set so an MCP server can never collide with a built-in.
fn build_registry(
    caps: config::Capabilities,
    creator: bool,
    disabled: &[String],
) -> Vec<tool::ToolDef> {
    let mut reg = tools::registry(caps);
    reg.extend(ask::tool_defs());
    reg.extend(goal::tool_defs());
    reg.extend(session::tool_defs());
    reg.extend(bg::tool_defs());
    reg.extend(instructions::tool_defs());
    reg.extend(todo::tool_defs());
    if creator {
        reg.extend(apps::tool_defs());
        reg.extend(skills::tool_defs());
    }
    reg.extend(mcp::tool_defs());
    let mut seen = std::collections::HashSet::new();
    reg.into_iter()
        .filter(|t| !disabled.iter().any(|d| d == t.name))
        .filter(|t| seen.insert(t.name))
        .collect()
}

/// System prompt for the current settings. Rebuilt at startup and before every
/// turn so mode, capabilities, MCP servers and skill catalog stay current.
/// Ordered static-first, dynamic-last to maximize provider prefix-cache reuse.
fn build_system_prompt(
    c: &config::Config,
    skill_list: &[skills::Skill],
    caps: config::Capabilities,
) -> String {
    let mut system_prompt = format!(
        "{SYSTEM_PROMPT}\n\nGOALS\n- `get_goal`, `create_goal`, `update_goal`: always call \
`get_goal` first to read the exact `id` and `revision` required by `update_goal`.\n- {}",
        goal::guidance()
    );
    if caps.web_tools {
        system_prompt.push_str(WEB_TOOLS_GUIDANCE);
    }
    if caps.speech {
        system_prompt.push_str(VOICE_GUIDANCE);
    }
    if !caps.web_tools || !caps.speech || !c.disabled_tools.is_empty() {
        system_prompt.push_str("\n\nCAPABILITIES");
        if !caps.web_tools {
            system_prompt.push_str(
                "\n- `search`, `fetch`, and `ocr` are unavailable on this third-party \
OpenAI-compatible endpoint. Use local tools (`bash`, `read_file`, `write_file`) instead, \
and do not claim you can search the web.",
            );
        }
        if !caps.speech {
            system_prompt.push_str(
                "\n- Speech (ASR/TTS) is unavailable on this endpoint; do not mention \
`speak` and do not claim you can listen or talk.",
            );
        }
        if !c.disabled_tools.is_empty() {
            system_prompt.push_str(&format!(
                "\n- Disabled by user config (not registered): {}. Do not attempt to call them.",
                c.disabled_tools.join(", ")
            ));
        }
    }
    let mcp_servers = mcp::server_summary();
    if !mcp_servers.is_empty() {
        let names = mcp_servers
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        system_prompt.push_str(&format!(
            "\n\nMCP TOOLS\n- External MCP servers connected: {names}. Tools are exposed as \
`mcp__<server>__<tool>` with their own JSON schemas; prefer them for the external systems \
they cover."
        ));
    }
    // UI language -> default answer language. The user's explicit request wins.
    system_prompt.push_str(&format!(
        "\n\nLANGUAGE\n- The interface language is {} ({}). Write every reply in it by \
default: prose, artifact bodies, state reports, questions, and errors.\n- The user's own \
request wins: if they ask for another language, or tell you to match the language they \
write in, switch to it and keep using it.",
        i18n::language_name(),
        i18n::language_native()
    ));
    // Dynamic tail (changes across projects/turns): kept last for prefix caching.
    system_prompt.push_str(&instructions::prompt_section());
    let skills_block = skills::catalog_prompt(skill_list);
    if !skills_block.is_empty() {
        if !system_prompt.ends_with('\n') {
            system_prompt.push('\n');
        }
        system_prompt.push_str(&skills_block);
    }
    if c.creator {
        system_prompt.push_str(&apps::prompt_section());
    }
    system_prompt
}

fn main() {
    // A panic must not leave the panel full of half-drawn glyphs and escape
    // sequences: restore the terminal first, then report.
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write as _;
        crate::stty_tty(&["sane", "min", "1", "time", "0"]);
        let mut o = std::io::stdout();
        let _ = o.write_all(b"\x1b[0m\x1b[?25h\x1b[2J\x1b[H");
        let _ = o.flush();
        eprintln!("dst: internal error: {info}");
    }));

    // Host/device debug entry points for the real DSH login flow.
    let argv: Vec<String> = std::env::args().collect();
    // Release/CI smoke check and a quick way for users to identify a build.
    if argv.get(1).map(|s| s.as_str()) == Some("--version")
        || argv.get(1).map(|s| s.as_str()) == Some("-V")
    {
        let sha = crate::update::current_sha256()
            .map(|s| format!(", sha256:{}", crate::update::short_sha(&s)))
            .unwrap_or_default();
        println!(
            "dst {} ({}, {}{sha})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH,
            if cfg!(debug_assertions) { "debug" } else { "release" }
        );
        return;
    }
    if argv.get(1).map(|s| s.as_str()) == Some("--pet-preview") {
        std::process::exit(pet_ui::preview(&argv));
    }
    if argv.iter().any(|a| a == "--dsh-login-test") {
        let port = 7788u16;
        let mut never_cancel = || false;
        let result = dsh_login::run(port, &mut |ev| match ev {
            dsh_login::Event::Phase(p) => eprintln!("phase: {p}"),
            dsh_login::Event::Message(m) => eprintln!("msg: {m}"),
            dsh_login::Event::Step(s) => eprintln!("step: {s}"),
            dsh_login::Event::WebToken(_) => eprintln!("step: web token received"),
            dsh_login::Event::Qr(bytes) => {
                eprintln!("qr: {} bytes", bytes.len());
                match dsh_login::qr_matrix_from_jpeg(&bytes) {
                    Ok(m) => eprintln!("qr matrix: {}x{}", m.len(), m.len()),
                    Err(e) => eprintln!("qr matrix: {e}"),
                }
            }
        }, &mut never_cancel);
        match result {
            Ok(token) => {
                println!("TOKEN {}", dsh_login::mask(&token));
                match dsh_login::verify_token(&token) {
                    Ok(reply) => println!("VERIFY OK {reply}"),
                    Err(e) => println!("VERIFY FAILED {e}"),
                }
            }
            Err(e) => {
                println!("LOGIN FAILED {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if argv.get(1).map(|s| s.as_str()) == Some("--login-server-test") {
        let port: u16 = argv.get(2).and_then(|p| p.parse().ok()).unwrap_or(8899);
        let lang = argv.get(3).map(|s| s.as_str()).unwrap_or("en");
        match login::start(port, lang) {
            Ok(session) => {
                println!("URL {}", session.url);
                let t0 = std::time::Instant::now();
                let mut announced_cancel = false;
                loop {
                    if let Ok(sub) = session.rx.try_recv() {
                        if let Some(t) = sub.token {
                            println!("TOKENPASTE {}", dsh_login::mask(&t));
                        } else {
                            println!("MANUAL {} {}", sub.base, dsh_login::mask(&sub.key));
                        }
                        return;
                    }
                    let (token, err, phase) = {
                        let st = session.shared.lock().unwrap();
                        (st.token.clone(), st.error.clone(), st.phase.clone())
                    };
                    if let Some(t) = token {
                        println!("DONE {} phase={phase}", dsh_login::mask(&t));
                        return;
                    }
                    if let Some(e) = err {
                        if e.to_lowercase().contains("cancelled") {
                            if !announced_cancel {
                                println!("CANCELLED");
                                announced_cancel = true;
                            }
                        } else {
                            println!("FAILED {e}");
                            std::process::exit(1);
                        }
                    }
                    if t0.elapsed().as_secs() > 120 {
                        println!("TIMEOUT");
                        std::process::exit(1);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            Err(e) => {
                println!("SERVER FAILED {e}");
                std::process::exit(1);
            }
        }
    }
    if argv.get(1).map(|s| s.as_str()) == Some("--login-page") {
        let lang = argv.get(2).map(|s| s.as_str()).unwrap_or("en");
        let html = login::page_for_test(lang);
        println!("LEN {} LANG {}", html.len(), lang);
        std::fs::write("/tmp/login-page.html", &html).ok();
        return;
    }
    if argv.get(1).map(|s| s.as_str()) == Some("--qr-ppm") {
        let (src, dst) = (argv.get(2), argv.get(3));
        if let (Some(src), Some(dst)) = (src, dst) {
            match std::fs::read(src) {
                Ok(bytes) => match dsh_login::qr_matrix_from_jpeg(&bytes) {
                    Ok(m) => match login::matrix_ppm(&m, Some((156, 0, 164, 170))) {
                        Ok(ppm) => {
                            let _ = std::fs::write(dst, ppm);
                            println!("PPM {}x{}", m.len(), m.len());
                        }
                        Err(e) => println!("PPM FAILED {e}"),
                    },
                    Err(e) => println!("PPM FAILED {e}"),
                },
                Err(e) => println!("PPM FAILED read: {e}"),
            }
        } else {
            println!("usage: dst --qr-ppm <qr.jpg> <out.ppm>");
        }
        return;
    }
    if argv.get(1).map(|s| s.as_str()) == Some("--qr-matrix") {
        match argv.get(2) {
            Some(path) => match std::fs::read(path) {
                Ok(bytes) => match dsh_login::qr_matrix_from_jpeg(&bytes) {
                    Ok(m) => {
                        println!("MATRIX {}x{}", m.len(), m.first().map(|r| r.len()).unwrap_or(0));
                        for row in m.iter().take(3) {
                            println!("{row}");
                        }
                    }
                    Err(e) => println!("MATRIX FAILED {e}"),
                },
                Err(e) => println!("MATRIX FAILED read: {e}"),
            },
            None => println!("usage: dst --qr-matrix <qr.jpg>"),
        }
        return;
    }
    // Housekeeping before the TUI starts: sessions and scratch files were
    // written every turn/action and never pruned.
    config::prune_sessions(40);
    config::prune_tmp(7 * 24 * 3600);
    let config_rc: ConfigRc = std::rc::Rc::new(std::cell::RefCell::new(config::Config::load()));
    i18n::set(&config_rc.borrow().language);
    let mut cfg = api_cfg(&config_rc.borrow());
    tools::set_cfg(cfg.clone());
    voice::set_cfg(cfg.clone());
    let shared_cfg = std::rc::Rc::new(std::cell::RefCell::new(cfg.clone()));
    // Skills first (their catalog goes into the prompt), then MCP servers
    // (their tools are discovered over JSON-RPC at startup).
    usage::load();
    jobs::set_compact_settings(crate::compact::Settings::from_config(&config_rc.borrow()));
    // The hardware notebook is a normal skill package that ships with dst:
    // install the seed before the catalog is read so the model can use it from
    // the very first turn (a scanned/refined copy is never overwritten).
    let hw_seeded = hwinfo::seed();
    let mut skill_list = skills::load_all();
    if hw_seeded {
        skill_list = skills::load_all();
    }
    tools::set_skills(skill_list.clone());
    mcp::install(mcp::start_all());
    // Every return path (diagnostics included) must kill the MCP servers it
    // started; a dropped Child is not reaped and would outlive dst.
    let _mcp_guard = mcp::ShutdownOnDrop;
    let mut caps_at_start = config_rc.borrow().capabilities();
    let mut creator_at_start = config_rc.borrow().creator;
    let disabled_at_start = config_rc.borrow().disabled_tools.clone();
    let shared_reg = std::rc::Rc::new(std::cell::RefCell::new(build_registry(
        caps_at_start,
        creator_at_start,
        &disabled_at_start,
    )));
    let system_prompt = build_system_prompt(&config_rc.borrow(), &skill_list, caps_at_start);
    let shared_tdefs = std::rc::Rc::new(std::cell::RefCell::new(tool::definitions(
        &shared_reg.borrow(),
    )));

    let argv: Vec<String> = std::env::args().collect();
    if argv.len() >= 2 && argv[1] == "--voice-selftest" {
        voice::set_cfg(cfg.clone());
        jobs::set_compact_settings(crate::compact::Settings::from_config(&config_rc.borrow()));
        let secs = argv.get(2).and_then(|s| s.parse().ok()).unwrap_or(5u32);
        match voice::mic_probe(secs) {
            Ok((rms, text)) => println!("rms={rms:.4}\ntranscript={text:?}"),
            Err(e) => println!("error: {e}"),
        }
        return;
    }
    if argv.len() >= 2 && argv[1] == "--hwinfo-scan" {
        // Diagnostics: run the hardware scan exactly as /update-hwinfo does.
        // Safe on the panel (it exits without touching the terminal UI).
        match hwinfo::update() {
            Ok(s) => println!(
                "path={} version={} sections={} bytes={} changed={} secs={:.1}",
                s.path, s.version, s.sections, s.bytes, s.changed, s.secs
            ),
            Err(e) => {
                println!("HWINFO FAILED {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if argv.len() >= 2 && argv[1] == "--tools" {
        // Diagnostics: the exact tool set and capability decision.
        let caps = config_rc.borrow().capabilities();
        let creator = config_rc.borrow().creator;
        println!("base={}", config_rc.borrow().base);
        println!("official={}", crate::config::is_official(&config_rc.borrow().base));
        println!("web_tools={} speech={}", caps.web_tools, caps.speech);
        println!("mode={}", if creator { "creator" } else { "normal" });
        let disabled = config_rc.borrow().disabled_tools.clone();
        let mode_reg = build_registry(caps, creator, &disabled);
        let mut names: Vec<&str> = mode_reg.iter().map(|t| t.name).collect();
        names.sort();
        println!("tools={}", names.join(","));
        return;
    }
    if argv.len() >= 4 && argv[1] == "--call" {
        // Diagnostics: execute one tool call exactly as the model would.
        let call = tool::Call {
            id: "cli".to_string(),
            name: argv[2].clone(),
            raw_args: argv[3].clone(),
        };
        let mut host = tool::NullHost;
        let outs = tool::run_batch(&shared_reg.borrow(), &[call], &mut host);
        for o in outs {
            println!("{}", o.content);
            if o.is_error {
                std::process::exit(2);
            }
        }
        return;
    }
    if argv.len() >= 3 && argv[1] == "--once" {
        run_once(&cfg, &shared_reg.borrow(), &system_prompt, &argv[2]);
        return;
    }

    // The TUI owns the terminal; diagnostics (--tools/--once) run without it.
    if !single_instance() {
        eprintln!(
            "dst: this terminal already runs a dst instance; \
start another one in a different terminal or tmux window"
        );
        std::process::exit(1);
    }

    let mut scr = Scr::new();
    tools::set_panel_size(scr.rows, scr.cols);
    if let Ok(text) = std::fs::read_to_string(config::history_path()) {
        for line in text.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with("/key") {
                scr.history.push(line.to_string());
            }
        }
    }
    scr.speech_ok = config_rc.borrow().capabilities().speech;
    scr.auto_send = config_rc.borrow().auto_send_voice;
    scr.creator = config_rc.borrow().creator;
    if scr.creator {
        scr.refresh_apps_count();
    }
    let mut session_path: Option<std::path::PathBuf> = None;
    raw_mode(true);
    crate::apps::set_screen_available(true);
    crate::ask::set_screen_available(true);

    scr.pet = Some(std::rc::Rc::new(std::cell::RefCell::new(pet_ui::Host::boot(
        &config_rc.borrow(),
    ))));
    scr.seed_splash(&cfg.model);
    onboarding(&mut scr, &config_rc);
    maybe_show_tips(&mut scr, &config_rc);
    flush_onboard_notice(&mut scr);
    scr.startup_notice = Some(startup_version_notice(&config_rc.borrow()));
    cfg = api_cfg(&config_rc.borrow());
    *shared_cfg.borrow_mut() = cfg.clone();
    tools::set_cfg(cfg.clone());
    voice::set_cfg(cfg.clone());
    refresh_ctx_window(&mut scr, &cfg);
    // Startup self-update check (release builds only): background thread, the
    // TUI offers the result once it lands.
    if config_rc.borrow().configured {
        crate::update::spawn_check(crate::update::Channel::parse(
            &config_rc.borrow().update_channel,
        )
        .unwrap_or(crate::update::Channel::Nightly));
    }
    if !config_rc.borrow().configured {
        scr.push(i18n::t("unconfigured"), K_ERR);
    }
    let msgs: jobs::Msgs = std::rc::Rc::new(std::cell::RefCell::new(vec![harness::msg("system", &system_prompt)]));
    {
        let reg2 = shared_reg.clone();
        let tdefs2 = shared_tdefs.clone();
        let msgs2 = msgs.clone();
        let sc = shared_cfg.clone();
        scr.on_tick = Some(Box::new(move |s: &mut Scr| {
            let c = sc.borrow().clone();
            jobs::poll(&c, &reg2.borrow(), &tdefs2.borrow(), &msgs2, s);
            if let Some(Ok(Some(info))) = crate::update::take_check() {
                s.update_offer = Some(info);
            }
            s.watch_buffer.extend(crate::bg::poll_watchers());
            // Coalesce a chatty watcher into one bounded message and rate-limit
            // how often lines can start an unattended turn.
            let due = s
                .last_watch_trigger
                .map(|t| t.elapsed().as_millis() >= 1200)
                .unwrap_or(true);
            if due && !s.watch_buffer.is_empty() {
                let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
                for (id, line) in s.watch_buffer.drain(..) {
                    match grouped.iter_mut().find(|(g, _)| *g == id) {
                        Some((_, lines)) => lines.push(line),
                        None => grouped.push((id, vec![line])),
                    }
                }
                for (id, lines) in grouped {
                    let total = lines.len();
                    let shown: Vec<String> = lines.iter().rev().take(5).rev().cloned().collect();
                    let mut text = format!("[watcher {id}] {}", shown.join("\n[watcher {id}] "));
                    if total > shown.len() {
                        text.push_str(&format!("\n(+{} more lines)", total - shown.len()));
                    }
                    s.auto_queue.push((
                        text,
                        format!(
                            "* Watcher {id}: {} ({} lines)",
                            first_line_short(shown.last().map(|x| x.as_str()).unwrap_or(""), 26),
                            total
                        ),
                    ));
                }
                s.last_watch_trigger = Some(std::time::Instant::now());
            }
        }));
    }

    let mut turn_no = 0u32;
    // A pending automatic goal round: (model prompt, one-line transcript label).
    let mut goal_seed: Option<(String, String)> = None;
    // A queued line pulled out of the busy-time queue so the outer loop can
    // dispatch it as a command if it starts with `/` (a queued `/clear` must
    // not be sent to the model as chat).
    let mut pending_line: Option<String> = None;
    loop {
        // A queued command is a human instruction: it must run before the
        // goal driver spends another autonomous round (DSH's driver waits for
        // competing queued prompts instead of starving them).
        let (line, label) = if let Some(q) = pending_line.take() {
            (q, None)
        } else {
            match goal_seed.take() {
                Some((prompt, label)) => (prompt, Some(label)),
                None => match scr.edit() {
                    Some(l) => (l, None),
                    None => break,
                },
            }
        };
        // Slash commands are human commands: they run here and never become a
        // model message (the same contract as DSH's command registry).
        if label.is_none() {
            scr.history_push(&line);
        }
        if label.is_none() && line.starts_with('/') {
            let goal_before = goal::current();
            // Bind the borrow before the if-let: a temporary RefMut in the
            // scrutinee would live for the whole body and make every later
            // config access (wizard, login, save) panic.
            let dispatched = {
                let mut guard = config_rc.borrow_mut();
                cmd::dispatch(&line, &mut guard)
            };
            if let Some(out) = dispatched {
                // `/btw` is an ephemeral side question: keep the main transcript
                // (and opening splash) completely untouched.
                if out.btw.is_none() {
                    scr.push(&format!("> {line}"), K_USER);
                }
                if out.clear {
                    // /clear and /new start a new conversation, not just a new
                    // screen: fresh system prompt, no session file, zero
                    // context pressure. The old session stays on disk for
                    // /resume.
                    scr.seed_splash(&config_rc.borrow().model.clone());
                    let fresh = {
                        let c = config_rc.borrow();
                        let caps = c.capabilities();
                        let prompt = build_system_prompt(&c, &skill_list, caps);
                        vec![harness::msg("system", &prompt)]
                    };
                    *msgs.borrow_mut() = fresh;
                    session::clear_current();
                    session_path = None;
                    crate::usage::reset_context();
                    crate::todo::clear();
                }
                for (text, kind) in out.lines {
                    scr.push(&text, kind);
                }
                if let Some(arg) = out.pet.clone() {
                    let mut guard = config_rc.borrow_mut();
                    apply_pet(&mut scr, &mut guard, &arg);
                }
                if out.quit {
                    break;
                }
                if out.compact {
                    // Manual /compact: DSH's `compactNow` bypasses the pressure
                    // threshold and keeps no verbatim tail.
                    let mut manual = {
                        let c = config_rc.borrow();
                        crate::compact::Settings::from_config(&c)
                    };
                    manual.threshold_tokens = 0;
                    manual.retain_ratio = 0.0;
                    cfg = api_cfg(&config_rc.borrow());
                    refresh_ctx_window(&mut scr, &cfg);
                    let defs = shared_tdefs.borrow().clone();
                    let window = scr.ctx_window;
                    let result = {
                        let btw_ctx = btw_snapshot(&msgs.borrow(), None);
                        let mut m = msgs.borrow_mut();
                        let mut ui = TermUi {
                            scr: &mut scr,
                            acc: String::new(),
                            think_cur: String::new(),
                            think_wrapped: Vec::new(),
                            steer: Vec::new(),
                            tools: std::collections::HashMap::new(),
                            spin: 0,
                            interrupted: false,
                            cfg_rc: config_rc.clone(),
                            btw_ctx,
                        };
                        crate::compact::compact(&cfg, &manual, &mut m, window, &defs, &mut ui)
                    };
                    // The manual path call may leave the live status row set
                    // when it returns early (e.g. "nothing to compact").
                    scr.live = None;
                    match result {
                        Ok(r) => scr.push(
                            &i18n::tf(
                                "ui_compact_done",
                                &[
                                    &crate::compact::human_tokens(r.before_tokens),
                                    &crate::compact::human_tokens(r.after_tokens),
                                    &r.dropped_messages.to_string(),
                                    &r.pruned.to_string(),
                                ],
                            ),
                            K_DIM,
                        ),
                        Err(e) => scr.push(&i18n::tf("ui_compaction_failed", &[&e.to_string()]), K_ERR),
                    }
                }
                if out.login {
                    if start_login(&mut scr, &config_rc) {
                        cfg = api_cfg(&config_rc.borrow());
                        *shared_cfg.borrow_mut() = cfg.clone();
                        tools::set_cfg(cfg.clone());
                        voice::set_cfg(cfg.clone());
        jobs::set_compact_settings(crate::compact::Settings::from_config(&config_rc.borrow()));
                        refresh_ctx_window(&mut scr, &cfg);
                        maybe_show_tips(&mut scr, &config_rc);
                    }
                }
                if out.config_wizard {
                    let creator_before = config_rc.borrow().creator;
                    config_menu(&mut scr, &config_rc);
                    if creator_before != config_rc.borrow().creator {
                        let n = crate::apps::load().len().to_string();
                        if config_rc.borrow().creator {
                            let dir = crate::config::home_shorten(
                                &crate::apps::apps_dir().display().to_string(),
                            );
                            scr.push(&i18n::tf("cmd_creator_on", &[&dir, &n]), K_NORM);
                        } else {
                            scr.push(&i18n::tf("cmd_creator_off", &[&n]), K_DIM);
                        }
                        scr.refresh_apps_count();
                    }
                }
                if out.model_menu {
                    model_menu(&mut scr, &config_rc);
                }
                if out.help_menu {
                    if let Some(chosen) = help_menu(&mut scr) {
                        pending_line = Some(chosen);
                    }
                }
                if out.rewind_menu {
                    if let Some(prompt) = rewind_menu(&mut scr, &msgs) {
                        if !prompt.trim().is_empty() {
                            scr.input = prompt.chars().collect();
                            scr.cursor = scr.input.len();
                            scr.push(i18n::t("rewind_prefilled"), K_DIM);
                        }
                        // Persist the rewound transcript right away: quitting
                        // here must not leave the old future in the session.
                        crate::usage::seed_context(crate::compact::estimate_tokens(
                            &msgs.borrow(),
                        ));
                        if let Some(p) = &session_path {
                            save_session(&msgs, &config_rc.borrow(), p);
                        }
                        scr.draw();
                    }
                }
                if out.extensions_menu {
                    extensions_menu(&mut scr, &config_rc);
                }
                if out.resume_menu {
                    if let Some(path) = resume_menu(&mut scr) {
                        if let Some(p) = resume_session_path(&mut scr, &msgs, &path) {
                            session_path = Some(p);
                            crate::usage::seed_context(crate::compact::estimate_tokens(
                                &msgs.borrow(),
                            ));
                        }
                    }
                }
                i18n::set(&config_rc.borrow().language);
                // The config wizard edits these live; the screen must follow
                // immediately instead of keeping the startup values.
                scr.auto_send = config_rc.borrow().auto_send_voice;
                let caps_now = config_rc.borrow().capabilities();
                let creator_now = config_rc.borrow().creator;
                if caps_now.web_tools != caps_at_start.web_tools
                    || caps_now.speech != caps_at_start.speech
                    || creator_now != creator_at_start
                {
                    caps_at_start = caps_now;
                    creator_at_start = creator_now;
                    // The registry follows both capabilities and the creator
                    // mode: the app_* tools exist only while it is on.
                    let disabled = config_rc.borrow().disabled_tools.clone();
                    let next_reg = build_registry(caps_at_start, creator_at_start, &disabled);
                    *shared_tdefs.borrow_mut() = tool::definitions(&next_reg);
                    *shared_reg.borrow_mut() = next_reg;
                    scr.speech_ok = caps_now.speech;
                }
                scr.creator = creator_at_start;
                cfg = api_cfg(&config_rc.borrow());
                *shared_cfg.borrow_mut() = cfg.clone();
                tools::set_cfg(cfg.clone());
                voice::set_cfg(cfg.clone());
        jobs::set_compact_settings(crate::compact::Settings::from_config(&config_rc.borrow()));
                // A command may have paused/cleared the goal while a round was
                // already planned: do not run that round.
                if goal_seed.is_some()
                    && !goal::current().map(|g| g.is_active()).unwrap_or(false)
                {
                    goal_seed = None;
                }
                // `/apps run <name>` hands the app to the screen; `/apps` opens
                // the manager. Both own the panel, so run them last.
                if let Some(name) = out.run_app.clone() {
                    match crate::apps::find(&name) {
                        Some(app) => {
                            match crate::apps::run_on_screen(&app, crate::apps::DIRECT_TIMEOUT_S) {
                                Ok(_) => scr.push(
                                    &i18n::tf("apps_exited", &[&app.label()]),
                                    K_DIM,
                                ),
                                Err(e) => scr.push(&e, K_ERR),
                            }
                        }
                        None => scr.push(&i18n::tf("apps_not_found", &[&name]), K_ERR),
                    }
                }
                if out.apps_menu {
                    match apps_menu(&mut scr, &config_rc) {
                        AppsAction::RunPrompt(label, prompt) => {
                            if prompt.trim().is_empty() {
                                scr.push(&i18n::tf("apps_prompt_empty", &[&label]), K_ERR);
                            } else {
                                scr.prompt_label = Some(i18n::tf("apps_prompt_run", &[&label]));
                                pending_line = Some(format!(
                                    "Run the prompt app '{label}' for me and follow its instructions.\n\n{}",
                                    prompt.trim()
                                ));
                            }
                        }
                        AppsAction::NewApp => {
                            // Seed the prompt so the user only types what the app
                            // should do; the model (creator mode) builds it.
                            scr.input = i18n::t("apps_new_prompt").chars().collect();
                            scr.cursor = scr.input.len();
                            scr.push(&format!("+ {}", i18n::t("apps_new_hint")), K_DIM);
                        }
                        AppsAction::Improve(label) => {
                            // Seed an edit request for the selected app.
                            scr.input = i18n::tf("apps_fix_prompt", &[&label]).chars().collect();
                            scr.cursor = scr.input.len();
                            scr.push(&format!("+ {}", i18n::t("apps_fix_hint")), K_DIM);
                        }
                        AppsAction::None => {}
                    }
                    scr.refresh_apps_count();
                }
                // A command may hand work to the model (/update-hwinfo in
                // creator mode): dispatch it as the next turn's request.
                if out.image_clear {
                    scr.attachments.clear();
                    scr.push(i18n::t("ui_image_cleared"), K_DIM);
                }
                if let Some(arg) = out.image {
                    let max = crate::vision::MAX_PER_MESSAGE;
                    if scr.attachments.len() >= max {
                        scr.push(
                            &i18n::tf("ui_image_too_many", &[&max.to_string()]),
                            K_ERR,
                        );
                    } else {
                        match crate::vision::Attachment::load(&arg) {
                            Ok(a) => {
                                scr.push(
                                    &i18n::tf("ui_image_added", &[&a.label()]),
                                    K_NORM,
                                );
                                scr.attachments.push(a);
                            }
                            Err(e) => {
                                scr.push(&i18n::tf("ui_image_failed", &[&e]), K_ERR)
                            }
                        }
                    }
                }
                if let Some(question) = out.btw {
                    let question = question.trim().to_string();
                    if question.is_empty() {
                        scr.push(i18n::t("cmd_btw_usage"), K_ERR);
                    } else {
                        let caps = config_rc.borrow().capabilities();
                        let now_cfg = api_cfg(&config_rc.borrow());
                        let btw_opts = harness::RunOpts::from_config(
                            &config_rc.borrow(),
                            scr.ctx_window,
                        );
                        run_btw(&mut scr, &now_cfg, caps, &msgs, &question, &btw_opts, None);
                        scr.draw();
                    }
                }
                if let Some(arg) = out.update {
                    run_update_command(&mut scr, &config_rc, &arg);
                }
                if let Some(p) = out.model_prompt {
                    // The request is internal: show a short label in the
                    // transcript instead of the whole prompt body.
                    scr.prompt_label = Some(i18n::t("hw_refining").to_string());
                    pending_line = Some(p);
                }
                scr.draw();
                // `/goal <objective>`, `/goal resume`, `/goal edit` re-arm the
                // driver: start the next automatic round right away.
                if let Some(after) = goal::current() {
                    let changed = match &goal_before {
                        None => true,
                        Some(b) => {
                            b.id != after.id
                                || b.armed != after.armed
                                || b.revision != after.revision
                        }
                    };
                    if changed && after.is_active() {
                        if let Some((before, round)) = goal::start_round() {
                            goal_seed = Some((
                                goal::round_prompt(&before, round),
                                format!("\u{27f3} {}", i18n::tf("ui_goal_round", &[&round.to_string()])),
                            ));
                        }
                    }
                }
                continue;
            }
        }
        let mut next: Option<(String, Option<String>)> = Some((line, label));
        let mut last_report: Option<harness::TurnReport> = None;
        while let Some((submit, label)) = next {
            // The splash is for the idle screen only: once the user starts
            // working, give the whole transcript area to the conversation.
            if !scr.splash_done {
                scr.lines.clear();
                scr.scroll = 0;
                scr.splash_done = true;
                scr.art_on = false;
            }
            match scr.auto_label.take() {
                Some(l) => scr.push(&l, K_TOOL),
                None => match &label {
                    Some(l) => scr.push(l, K_TOOL),
                    None => match scr.prompt_label.take() {
                        Some(pl) => scr.push(&format!("> {pl}"), K_USER),
                        None => scr.push(&format!("> {submit}"), K_USER),
                    },
                },
            }
            // Human turns may carry local images; goal/auto rounds never do.
            let images = if label.is_none() {
                std::mem::take(&mut scr.attachments)
            } else {
                Vec::new()
            };
            for a in &images {
                scr.push(&format!("  {}", a.placeholder()), K_DIM);
            }
            turn_no += 1;
            // DSH clears its todo projection at every turn/start: a plan is for
            // the work in front of the model, not a stale cross-turn list.
            crate::todo::clear();
            scr.turn_tools.clear();
            // A session must exist before the turn so `rename` works inside
            // the very first turn and the file is there for /resume.
            if session_path.is_none() {
                let p = new_session_path();
                session::set_current(p.clone());
                let _ = save_session(&msgs, &config_rc.borrow(), &p);
                session_path = Some(p);
            }
            cfg = api_cfg(&config_rc.borrow());
            refresh_ctx_window(&mut scr, &cfg);
            // Skills can be written by the model in creator mode: re-read the
            // catalog every turn so a new skill shows up immediately.
            skill_list = skills::load_all();
            tools::set_skills(skill_list.clone());
            // Rebuild the system prompt each turn: creator mode, the installed
            // app catalog and the skill catalog must all be current.
            {
                let c = config_rc.borrow();
                let caps = c.capabilities();
                let prompt = build_system_prompt(&c, &skill_list, caps);
                drop(c);
                if let Some(first) = msgs.borrow_mut().first_mut() {
                    *first = harness::msg("system", &prompt);
                }
            }
            let mut opts = harness::RunOpts::from_config(&config_rc.borrow(), scr.ctx_window);
            opts.turn = turn_no;
            if label.is_some() {
                // Goal rounds use their own step budget (0 = unlimited).
                opts.max_steps = config_rc.borrow().goal_max_steps;
            }
            {
                let m = msgs.borrow();
                // The prompt this turn is about to run: rewind lists it so the
                // user picks a real previous message, not a turn number.
                let stamp_prompt = if label.is_none() { submit.as_str() } else { "" };
                crate::checkpoint::save(&m, &session::title(), stamp_prompt);
            }
            let in_goal_round = label.is_some();
            goal::set_in_round(in_goal_round);
            scr.busy = true;
            poll_mode(true);
            {
                // Snapshot before the mutable borrow, including the current
                // prompt so a mid-turn `/btw` knows what this turn is doing.
                let btw_ctx = btw_snapshot(&msgs.borrow(), Some(&submit));
                let mut m = msgs.borrow_mut();
                let mut ui = TermUi {
                    scr: &mut scr,
                    acc: String::new(),
                    think_cur: String::new(),
                    think_wrapped: Vec::new(),
                    steer: Vec::new(),
                    tools: std::collections::HashMap::new(),
                    spin: 0,
                    interrupted: false,
                    cfg_rc: config_rc.clone(),
                    btw_ctx,
                };
                let live_reg = shared_reg.borrow();
                last_report = Some(harness::run_turn(
                    &cfg, &opts, &mut m, &live_reg, &submit, &images, &mut ui,
                ));
            }
            goal::set_in_round(false);
            poll_mode(false);
            scr.busy = false;
            scr.live = None;
            scr.draw();
            // Creator mode: name what was saved, or say that nothing was.
            if config_rc.borrow().creator {
                scr.refresh_apps_count();
                let saved_apps = crate::skills::take_saved_apps();
                let saved_skills = crate::skills::take_saved();
                for name in &saved_apps {
                    scr.push(&i18n::tf("app_saved_notice", &[name]), K_DIM);
                }
                for (name, _path, version) in &saved_skills {
                    scr.push(
                        &i18n::tf("ui_skill_saved", &[name, &version.to_string()]),
                        K_DIM,
                    );
                }
                if saved_apps.is_empty() && saved_skills.is_empty() {
                    let mutated = scr.turn_tools.iter().any(|t| {
                        matches!(t.as_str(), "write_file" | "edit_file" | "skill_file")
                    });
                    if mutated {
                        scr.push(i18n::t("creator_nudge"), K_DIM);
                    }
                }
            }
            scr.turn_tools.clear();
            scr.draw();
            if scr.voice_pending || scr.pending_rec.is_some() {
                scr.voice_pending = false;
                let pre = scr.pending_rec.take();
                if scr.voice_capture(pre) {
                    let line: String = scr.input.iter().collect();
                    scr.input.clear();
                    scr.cursor = 0;
                    scr.queue.insert(0, line);
                }
            }
            if let Some(p) = &session_path {
                save_session(&msgs, &config_rc.borrow(), p);
            }
            // First human message becomes a fallback title until the model
            // calls `rename`; goal-round prompts are not human messages.
            if session::title().is_empty() {
                let fallback = if label.is_none() {
                    // First human message becomes a fallback title.
                    submit.split_whitespace().collect::<Vec<_>>().join(" ")
                } else {
                    // An autonomous goal round names the session after the goal.
                    goal::current()
                        .map(|g| g.objective)
                        .unwrap_or_default()
                };
                if !fallback.is_empty() {
                    let _ = session::rename(&fallback);
                }
            }
            if scr.quit {
                break;
            }
            next = match scr.queue.first() {
                None => None,
                Some(q) if q.starts_with('/') => {
                    pending_line = Some(scr.queue.remove(0));
                    None
                }
                Some(_) => Some((scr.queue.remove(0), None)),
            };
            if pending_line.is_some() {
                break;
            }
        }
        if scr.quit {
            break;
        }
        // ---- goal round driver (DSH `dsh-goal-round-driver`) ----
        // Esc stops the autonomous run: an interrupted round disarms the goal
        // instead of retrying it; `/goal resume` re-arms.
        let stopped_early = last_report
            .as_ref()
            .map(|r| r.cancelled || r.failed || r.limit_hit)
            .unwrap_or(false);
        if stopped_early {
            if goal::current().map(|g| g.is_active()).unwrap_or(false) {
                goal::set_armed(false);
                scr.push(i18n::t("ui_goal_disarmed"), K_DIM);
            }
        }
        if scr.queue.is_empty() && pending_line.is_none() {
            if let Some(goal) = goal::current() {
                if goal.is_active() {
                    if let Some((before, round)) = goal::start_round() {
                        goal_seed = Some((
                            goal::round_prompt(&before, round),
                            format!("\u{27f3} {}", i18n::tf("ui_goal_round", &[&round.to_string()])),
                        ));
                    } else if let Some(now) = goal::current() {
                        // `start_round` also returns None at the round cap and
                        // has already disarmed: report the fresh state so the
                        // user knows why the autonomous run stopped.
                        if now.max_rounds > 0 && now.rounds_started >= now.max_rounds {
                            scr.push(
                                &i18n::tf(
                                    "ui_goal_round_limit",
                                    &[&now.max_rounds.to_string()],
                                ),
                                K_DIM,
                            );
                        }
                    }
                }
            }
        }
    }


    if let Some(p) = &session_path {
        save_session(&msgs, &config_rc.borrow(), p);
    }
    crate::bg::shutdown();
    crate::mcp::shutdown();
    raw_mode(false);
    let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[2J\x1b[H");
    println!("bye");
}


fn first_lines(s: &str, n: usize) -> String {
    s.lines()
        .take(n)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
