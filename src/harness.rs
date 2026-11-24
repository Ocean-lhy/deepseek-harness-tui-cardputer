//! The turn/step driver, ported from DSH `core/agent-loop`.
//!
//! A **step** is one model request plus the tools it calls. A **turn** is zero
//! or more steps. Event order is DSH's, minus the seams that do not exist on a
//! 512 MB single-user device (no sandbox, no approval, no subagents):
//!
//! ```text
//! turn/start
//!   agent/pre-step            (context pressure -> compact claimed history)
//!   step/start
//!   request                   (prompt + step tool schema set)
//!   assistant/message         (thinking, then streamed content)
//!   tool/call*  -> tools/pre-execute -> tools/execute -> tools/result*
//!   additionalContexts        (FIFO, injected as user/message after results)
//!   step/end
//!     -> another step while the model owes a request
//! turn/end
//! ```
//!
//! The driver never touches the terminal. It talks to a [`Ui`], which is also
//! the place the keyboard is polled from, so typing stays live during a turn
//! and `Esc` can cancel it.

use crate::api::{self, ApiEv, Cfg, StreamOut, ToolCall};
use crate::json::J;
use crate::tool::{self, Call, PipeEvent, ToolDef};
use std::sync::mpsc::TryRecvError;
use std::time::Instant;

/// Everything that bounds one turn. `0` means "no limit" throughout: the user
/// asked for the old hard-coded step cap to be removed, with an opt-in config
/// key instead.
#[derive(Clone)]
pub struct RunOpts {
    pub max_steps: u32,
    pub compact: crate::compact::Settings,
    pub context_time: bool,
    /// Minimum seconds between clock readings (DSH default 600).
    pub context_refresh_secs: u64,
    /// 1-based turn number used by the clock-context text.
    pub turn: u32,
    /// Context window of the configured model, 0 when unknown.
    pub window: u64,
    /// Global tool timeout cap in ms; 0 = each tool's own `timeout_ms`.
    pub tool_timeout_ms: u64,
    /// Tool-call permission policy.
    pub sandbox: crate::sandbox::Settings,
    /// Transient-failure retries and fallback models.
    pub retries: u32,
    pub fallbacks: Vec<String>,
    /// Update the main conversation's context-pressure ring (`false` for `/btw`).
    pub update_context: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        RunOpts {
            max_steps: 0,
            compact: crate::compact::Settings::from_config(&crate::config::Config::default()),
            context_time: true,
            context_refresh_secs: 600,
            turn: 1,
            window: 0,
            tool_timeout_ms: 0,
            sandbox: crate::sandbox::Settings::default(),
            retries: 0,
            fallbacks: Vec::new(),
            update_context: true,
        }
    }
}

impl RunOpts {
    pub fn from_config(c: &crate::config::Config, window: u64) -> RunOpts {
        RunOpts {
            max_steps: c.max_steps,
            compact: crate::compact::Settings::from_config(c),
            context_time: c.context_time,
            context_refresh_secs: c.context_refresh_secs,
            turn: 1,
            window,
            tool_timeout_ms: c.tool_timeout_ms,
            sandbox: crate::sandbox::Settings::from_config(c),
            retries: c.api_retries,
            fallbacks: c.api_fallbacks.clone(),
            update_context: true,
        }
    }
}

/// What one turn did. The goal driver needs to know whether the turn ended by
/// itself (so the goal can be judged) or was interrupted/failed.
#[derive(Default)]
pub struct TurnReport {
    #[allow(dead_code)]
    pub steps: u32,
    /// The turn was cut by the configured step cap (model did not stop).
    pub limit_hit: bool,
    pub cancelled: bool,
    pub failed: bool,
    pub text: String,
}

/// What the UI wants done with the running turn.
#[derive(PartialEq, Eq)]
pub enum Action {
    Continue,
    Cancel,
}

#[allow(dead_code)] // events carry step numbers for future front ends
pub enum Ev {
    TurnStart,
    StepStart(u32),
    /// Transient model delta (`agent/assistant-stream` chunk).
    Delta(String),
    /// Live thinking progress. `chars` is how much has arrived so far.
    Thinking { secs: f32, chars: usize },
    /// A piece of the reasoning text, shown live and folded when content starts.
    ThinkingDelta(String),
    /// A tool body has been running for `secs` seconds (live progress ping).
    ToolTick { name: String, secs: f32 },
    /// Thinking finished; the UI should collapse it to one line.
    ThinkingDone { secs: f32, chars: usize },
    /// Settled assistant text (`assistant/message`).
    Assistant(String),
    /// `tool/call`, logged before execution. `title` is UI presentation only;
    /// `args` is the parsed argument JSON, which the UI expands into the card
    /// so the user can see exactly what is running.
    ToolCall {
        id: String,
        name: String,
        title: String,
        args: String,
    },
    /// `tool/result`, settled in model order.
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    /// Progress line from the `state_report` tool: one short sentence the TUI
    /// pins to the top row until the next transcript line arrives.
    StateReport(String),
    /// `additionalContexts` injected as a user message after the batch.
    Context(String),
    /// Automatic context compaction started for this many tokens.
    Compacting { tokens: u64, messages: usize },
    /// Compaction finished; `ok=false` means the history was left untouched.
    CompactingDone { ok: bool, note: String },
    /// The agent asked to take over the screen with a fixed-size paginated panel.
    Artifact(J),
    /// Harness-level notice (limit reached, failure, compaction).
    Note(String),
    /// Informational notice (compaction done, pruned, step budget reached).
    Info(String),
    /// Actionable recovery hint shown highlighted below an error.
    Hint(String),
    StepEnd(u32),
    TurnEnd { steps: u32, fate: TurnFate },
}

/// How the turn stopped. A failure or a cancel is not a successful turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnFate {
    Done,
    Failed,
    Cancelled,
}

/// Everything the driver needs from the front end.
pub trait Ui {
    fn on(&mut self, ev: Ev);
    /// Called continuously while the request is in flight. Implementations
    /// must not block for long: the keyboard is read here.
    fn pump(&mut self) -> Action;
    /// True once the interrupt belongs to the whole turn (Esc, voice), not
    /// just to the tool call that happened to be running. The driver checks it
    /// after every batch so Esc really stops the turn instead of handing
    /// cancelled tool results back to the model for another step.
    fn interrupted(&self) -> bool {
        false
    }
    /// Strict-mode permission pop-up. Returning false rejects the call.
    fn confirm(&mut self, request: &crate::sandbox::Approval) -> bool;
    /// A message typed while busy that should be injected at the next step
    /// boundary (DSH `steer`), not interrupt the turn.
    fn steer(&mut self) -> Option<String> {
        None
    }
    /// Interactive multiple-choice question prompt rendered on the panel.
    fn ask_user(
        &mut self,
        _req: &crate::ask::AskRequest,
    ) -> Result<crate::ask::AskResponse, String> {
        Err("ask_user needs the interactive screen (the TUI); this front end cannot prompt the user"
            .to_string())
    }
    /// Creator mode: hand the panel to a saved app until it exits, is stopped
    /// by a long-press of Esc, or times out. Front ends without an interactive
    /// screen reject the request instead of hanging.
    fn run_app(
        &mut self,
        _app: &crate::apps::App,
        _timeout_s: u64,
        _args: &str,
    ) -> Result<String, String> {
        Err("app_run needs the interactive screen (the TUI); this front end cannot take over the panel"
            .to_string())
    }
}

/// Bridges the tool executor's host callbacks back onto the UI, so a running
/// tool call can still deliver Esc.
struct UiHost<'a> {
    ui: &'a mut dyn Ui,
    api: Cfg,
    sandbox: crate::sandbox::Settings,
}

impl<'a> tool::ToolHost for UiHost<'a> {
    fn call(&mut self, ev: PipeEvent) {
        match ev {
            PipeEvent::Call { id, name, title, args } => {
                self.ui.on(Ev::ToolCall { id, name, title, args })
            }
            PipeEvent::Made(o) => {
                // A progress report is a UI event, not a tool card: surface it
                // the moment the call settles, before the batch moves on.
                let report = if o.name == "state_report" && !o.is_error {
                    o.value
                        .as_ref()
                        .and_then(|v| v.get("text"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };
                self.ui.on(Ev::ToolResult {
                    id: o.call_id,
                    name: o.name,
                    content: o.content,
                    is_error: o.is_error,
                });
                if let Some(text) = report {
                    self.ui.on(Ev::StateReport(text));
                }
            }
        }
    }
    fn cancel(&mut self) -> bool {
        // Service interactive screen requests (app_run / ask_user) before
        // pumping keys, so the modal opens within 15 ms.
        crate::apps::service_launch(self.ui);
        crate::ask::service_prompt(self.ui);
        self.ui.pump() == Action::Cancel
    }
    fn tick(&mut self, name: &str, secs: f32) {
        self.ui.on(Ev::ToolTick { name: name.to_string(), secs });
    }
    fn allow(&mut self, name: &str, args: &J) -> Result<(), String> {
        struct UiConfirmer<'a> {
            ui: &'a mut dyn Ui,
        }
        impl crate::sandbox::Confirmer for UiConfirmer<'_> {
            fn confirm(&mut self, request: &crate::sandbox::Approval) -> bool {
                self.ui.confirm(request)
            }
        }
        let mut confirmer = UiConfirmer { ui: &mut *self.ui };
        crate::sandbox::authorize(name, args, &self.sandbox, &self.api, &mut confirmer)
    }
}

pub fn msg(role: &str, content: &str) -> J {
    J::o(vec![("role", J::s(role)), ("content", J::s(content))])
}

/// A user message that carries local images as OpenAI/DeepSeek vision blocks:
/// `content` becomes an array of `text` + `image_url` (data URL) parts. Plain
/// text messages keep the cheap string form.
pub fn user_msg_with_images(input: &str, images: &[crate::vision::Attachment]) -> J {
    if images.is_empty() {
        return msg("user", input);
    }
    let mut parts: Vec<J> = Vec::new();
    let text = if input.trim().is_empty() {
        "What is in this image?".to_string()
    } else {
        input.to_string()
    };
    parts.push(J::o(vec![
        ("type", J::s("text")),
        ("text", J::s(&text)),
    ]));
    for a in images {
        parts.push(J::o(vec![
            ("type", J::s("image_url")),
            (
                "image_url",
                J::o(vec![("url", J::s(&a.data_url()))]),
            ),
        ]));
    }
    J::o(vec![("role", J::s("user")), ("content", J::Arr(parts))])
}

/// Replace a `read_image` tool result's image block with its text form once the
/// model has consumed it: keeps later steps and the saved history small, and
/// never re-bills the image.
fn strip_tool_images(msgs: &mut [J], pending: &mut Vec<(usize, String)>) {
    for (idx, text) in pending.drain(..) {
        if let Some(m) = msgs.get_mut(idx) {
            if let J::Obj(kv) = m {
                if let Some(slot) = kv.iter_mut().find(|(k, _)| k == "content") {
                    slot.1 = J::s(&text);
                }
            }
        }
    }
}

/// Replace the raw image blocks of the just-sent user message with a text
/// placeholder. The model already has the pictures; the history, session files
/// and every later request only need to know an image was there (the deepseek
/// API resizes to ~1300x1300 / <=1024 tokens per image anyway, and the encoded
/// payload would dominate every following step).
fn strip_attachments(
    msgs: &mut [J],
    idx: usize,
    input: &str,
    images: &[crate::vision::Attachment],
) {
    if images.is_empty() {
        return;
    }
    let Some(m) = msgs.get_mut(idx) else {
        return;
    };
    let mut text = input.trim().to_string();
    for a in images {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&a.placeholder());
    }
    if let J::Obj(kv) = m {
        if let Some(slot) = kv.iter_mut().find(|(k, _)| k == "content") {
            slot.1 = J::s(&text);
        }
    }
}

fn assistant_with_tools(text: &str, calls: &[ToolCall]) -> J {
    let mut arr = Vec::new();
    for c in calls {
        arr.push(J::o(vec![
            ("id", J::s(&c.id)),
            ("type", J::s("function")),
            (
                "function",
                J::o(vec![("name", J::s(&c.name)), ("arguments", J::s(&c.args))]),
            ),
        ]));
    }
    J::o(vec![
        ("role", J::s("assistant")),
        ("content", J::s(text)),
        ("tool_calls", J::Arr(arr)),
    ])
}

/// Actionable next step for a failed request, shown as a dim info line.
/// i18n key of the recovery hint for a provider error, if one is known.
fn actionable_hint(message: &str) -> Option<&'static str> {
    let m = message.to_ascii_lowercase();
    // A text-only endpoint or model rejects image blocks with a 400; point at
    // the two ways forward (vision model, or /ocr for text in images).
    if m.contains("image") {
        return Some("err_hint_image");
    }
    if m.contains("401") || m.contains("403") || m.contains("unauthorized")
        || m.contains("invalid api key") || m.contains("authentication")
    {
        Some("err_hint_auth")
    } else if m.contains("429") || m.contains("rate limit") || m.contains("quota") {
        Some("err_hint_rate")
    } else if is_transient(message) || m.contains("network") {
        Some("err_hint_network")
    } else {
        None
    }
}

/// Whether a provider error is a context-window overflow (`dsh-agent-loop`
/// treats this as an explicit compaction trigger).
/// Transient network/provider failures worth retrying (and switching model).
fn is_transient(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("curl")
        || m.contains("connection")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("502")
        || m.contains("503")
        || m.contains("504")
        || m.contains("429")
        || m.contains("rate limit")
        || m.contains("too many requests")
        || m.contains("network")
        || m.contains("stream closed")
        || m.contains("no data from api")
}

fn is_context_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("context length")
        || m.contains("context window")
        || m.contains("context_length_exceeded")
        || m.contains("maximum context")
        || m.contains("too many tokens")
        || (m.contains("prompt") && m.contains("too long"))
        || (m.contains("exceed") && m.contains("token"))
}

/// Verbatim DSH `dsh-repeat-tool-reminder` text (first threshold and later
/// thresholds differ; both are reproduced).
fn repeat_reminder(name: &str, args: &str, count: u32) -> String {
    if count <= 3 {
        "You are repeating the exact same tool call with identical arguments. Carefully analyze the previous result before calling again: if the task is not complete, try a different approach or different arguments instead of repeating the call."
            .to_string()
    } else {
        let shown: String = args.chars().take(500).collect();
        let extra = args.chars().count().saturating_sub(500);
        let args_text = if extra > 0 {
            format!("{shown}\u{2026} (+{extra} more chars)")
        } else {
            shown
        };
        format!(
            "Repeated tool call detected:\n- tool: {name}\n- consecutive_calls: {count}\n- arguments: {args_text}\nThe repeated calls are not making progress. Do not call this tool with these exact arguments again. Inspect the latest result and choose a different action, different arguments, or finish the task if enough evidence has been gathered."
        )
    }
}

/// Serialise the transcript directly to JSON. Avoids the deep clone of the
/// whole message tree that `J::Arr(msgs.clone()).dump()` used to make on every
/// step (the context can be hundreds of KB near the limit).
pub(crate) fn serialize_msgs(msgs: &[J]) -> String {
    let mut s = String::with_capacity(msgs.len() * 192);
    s.push('[');
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&m.dump());
    }
    s.push(']');
    s
}

/// One streamed request. Drains the API channel while pumping the UI, so the
/// user can keep typing and can cancel with Esc.
fn stream_once(
    cfg: &Cfg,
    messages_json: &str,
    defs: &J,
    ui: &mut dyn Ui,
) -> Result<StreamOut, (StreamOut, bool)> {
    let chat = match api::spawn_chat_json(cfg, messages_json, defs) {
        Ok(c) => c,
        Err(e) => {
            let mut o = StreamOut::default();
            o.finish = format!("spawn failed: {e}");
            return Err((o, false));
        }
    };

    let mut acc = String::new();
    let mut reason_chars = 0usize;
    // The clock starts with the request, not with the first reasoning token:
    // a model that emits no reasoning_content still gets a "thinking" line
    // instead of a silent screen.
    let mut think_t0: Option<Instant> = Some(Instant::now());
    let mut last_think: Option<Instant> = None;
    let cancelled = false;

    loop {
        let mut settled: Option<Result<StreamOut, String>> = None;
        loop {
            match chat.rx.try_recv() {
                Ok(ApiEv::Reason(r)) => {
                    reason_chars += r.chars().count();
                    ui.on(Ev::ThinkingDelta(r));
                }
                Ok(ApiEv::Content(c)) => {
                    if let Some(t0) = think_t0.take() {
                        ui.on(Ev::ThinkingDone {
                            secs: t0.elapsed().as_secs_f32(),
                            chars: reason_chars,
                        });
                    }
                    acc.push_str(&c);
                    ui.on(Ev::Delta(c));
                }
                Ok(ApiEv::Done(o)) => {
                    settled = Some(Ok(o));
                    break;
                }
                Ok(ApiEv::Failed(e)) => {
                    settled = Some(Err(e));
                    break;
                }
                Err(TryRecvError::Empty) => {
                    // Repaint the thinking line a few times a second.
                    if let Some(t0) = think_t0 {
                        let due = last_think
                            .map(|t| t.elapsed().as_millis() >= 150)
                            .unwrap_or(true);
                        if due {
                            ui.on(Ev::Thinking {
                                secs: t0.elapsed().as_secs_f32(),
                                chars: reason_chars,
                            });
                            last_think = Some(Instant::now());
                        }
                    }
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    settled = Some(Err("stream closed unexpectedly".into()));
                    break;
                }
            }
        }
        if let Some(r) = settled {
            if let Some(t0) = think_t0.take() {
                ui.on(Ev::ThinkingDone {
                    secs: t0.elapsed().as_secs_f32(),
                    chars: reason_chars,
                });
            }
            return match r {
                Ok(o) => Ok(o),
                Err(e) => {
                    let mut o = StreamOut::default();
                    o.text = acc;
                    o.finish = e;
                    Err((o, cancelled))
                }
            };
        }

        if !cancelled && ui.pump() == Action::Cancel {
            api::kill(chat.pid);
            ui.on(Ev::Info(crate::i18n::t("ui_cancelled").to_string()));
            // Keep draining until the killed curl settles, otherwise the
            // reader thread outlives the turn.
            let deadline = Instant::now();
            while deadline.elapsed().as_millis() < 2000 {
                if let Ok(ApiEv::Done(_)) | Ok(ApiEv::Failed(_)) = chat.rx.recv_timeout(
                    std::time::Duration::from_millis(50),
                ) {
                    break;
                }
            }
            let mut o = StreamOut::default();
            o.text = acc;
            o.finish = "cancelled".into();
            return Err((o, true));
        }

        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

/// Absolute history ceiling, independent of token accounting. Compaction is
/// the normal bound, but it can be disabled, fail, or compute an unbounded
/// budget when the endpoint reports no context window; without this a long
/// session grows until the device runs out of memory.
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

/// Cheap size estimate of one JSON value, without serializing it.
fn j_len(v: &J) -> usize {
    match v {
        J::Null => 4,
        J::Bool(_) => 5,
        J::Num(_) => 8,
        J::Str(s) => s.len() + 2,
        J::Arr(a) => 2 + a.iter().map(j_len).sum::<usize>(),
        J::Obj(kv) => 2 + kv.iter().map(|(k, v)| k.len() + 4 + j_len(v)).sum::<usize>(),
    }
}

/// Byte weight of one message for the hard cap. Image content blocks count as
/// a fixed ~8 KB (the API caps them at ~1024 tokens) instead of their base64
/// payload, so one attached picture cannot force an unrelated history trim.
fn msg_weight(m: &J) -> usize {
    let content = match m.get("content") {
        Some(J::Str(s)) => s.len() + 2,
        Some(J::Arr(parts)) => parts
            .iter()
            .map(|p| {
                if p.get("type").and_then(|t| t.as_str()) == Some("image_url") {
                    8 * 1024
                } else {
                    p.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.len() + 8)
                        .unwrap_or(8)
                }
            })
            .sum(),
        _ => 8,
    };
    let has_image = matches!(m.get("content"), Some(J::Arr(parts))
        if parts.iter().any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url")));
    if has_image {
        content + 128
    } else {
        j_len(m)
    }
}

fn approx_bytes(msgs: &[J]) -> usize {
    msgs.iter().map(msg_weight).sum()
}

/// Drop the oldest messages until the history fits in half the hard cap (so it
/// does not retrigger on every step). The system prompt stays, and the tail
/// never starts with an orphan tool result.
fn hard_trim(msgs: &mut Vec<J>) -> bool {
    if msgs.len() <= 3 || approx_bytes(msgs) <= MAX_HISTORY_BYTES {
        return false;
    }
    let target = MAX_HISTORY_BYTES / 2;
    let mut start = msgs.len();
    let mut tail = 0usize;
    while start > 1 {
        let add = msg_weight(&msgs[start - 1]);
        if tail + add > target {
            break;
        }
        tail += add;
        start -= 1;
    }
    while start < msgs.len() {
        let role = msgs[start].get("role").and_then(|r| r.as_str());
        if role == Some("tool") {
            start += 1;
        } else {
            break;
        }
    }
    if start <= 1 {
        return false;
    }
    msgs.drain(1..start);
    true
}

/// Run one turn: `input` is the claimed user message.
pub fn run_turn(
    cfg: &Cfg,
    opts: &RunOpts,
    msgs: &mut Vec<J>,
    reg: &[ToolDef],
    input: &str,
    images: &[crate::vision::Attachment],
    ui: &mut dyn Ui,
) -> TurnReport {
    ui.on(Ev::TurnStart);
    // Only the first request of the turn carries the image payload; the
    // message is rewritten to a placeholder as soon as a response arrives.
    let user_idx = msgs.len();
    msgs.push(user_msg_with_images(input, images));
    // Tool results from read_image keep their image block for exactly one
    // request (the one that lets the model look at it), then become text.
    let mut pending_tool_images: Vec<(usize, String)> = Vec::new();

    let defs = tool::definitions(reg);
    let mut steps = 0u32;
    let mut report = TurnReport::default();
    // DSH `dsh-repeat-tool-reminder`: identical consecutive calls are a loop,
    // not progress. Counted per turn, reset by any different call.
    let mut repeat_sig = String::new();
    let mut repeat_count = 0u32;

    loop {
        // `max_steps == 0` is unlimited (the old hard-coded cap of 12 is gone;
        // a looping model is instead bounded by Esc, the goal driver, or the
        // user's own config).
        if opts.max_steps > 0 && steps >= opts.max_steps {
            report.limit_hit = true;
            ui.on(Ev::Info(crate::i18n::tf(
                "ui_step_limit",
                &[&opts.max_steps.to_string()],
            )));
            break;
        }

        if ui.interrupted() {
            report.cancelled = true;
            ui.on(Ev::StepEnd(steps));
            break;
        }

        // Hard ceiling before compaction runs: even with compaction disabled
        // or failing, the process must stay bounded on a 446 MB device.
        if hard_trim(msgs) {
            ui.on(Ev::Info(crate::i18n::t("ui_history_trimmed").to_string()));
        }

        // agent/pre-step: keep the request it is about to send inside the
        // context window. This is token based (see compact.rs), not the old
        // message-count heuristic.
        crate::compact::maybe_compact(cfg, &opts.compact, msgs, opts.window, &defs, ui);

        // Steering: messages typed while busy are injected at this step
        // boundary, exactly like DSH `agent.steer` (next-step inbox).
        while let Some(s) = ui.steer() {
            let s = s.trim().to_string();
            if !s.is_empty() {
                ui.on(Ev::Context(s.clone()));
                msgs.push(msg("user", &s));
            }
        }

        // additionalContexts: ambient state is refreshed per request and is
        // rate-limited like DSH's `dsh-time-context`.
        if opts.context_time {
            crate::context::inject_time(
                msgs,
                opts.context_refresh_secs,
                opts.turn,
                steps + 1,
            );
        }

        steps += 1;
        ui.on(Ev::StepStart(steps));

        // Transient failures retry with backoff; if fallback models are
        // configured they are tried in order (DSH llm-retry style).
        let mut attempt = 0u32;
        let mut active = cfg.clone();
        let out = loop {
            let snapshot = serialize_msgs(msgs);
            match stream_once(&active, &snapshot, &defs, ui) {
                Ok(o) => {
                    if opts.update_context {
                        crate::usage::record_opt(o.usage.as_ref());
                    } else {
                        crate::usage::record_side_opt(o.usage.as_ref());
                    }
                    break Ok(o);
                }
                Err((partial, was_cancelled)) if !was_cancelled
                    && attempt < opts.retries
                    && is_transient(&partial.finish) =>
                {
                    attempt += 1;
                    if let Some(m) = opts.fallbacks.get(attempt as usize - 1) {
                        active.model = m.clone();
                    }
                    ui.on(Ev::Info(crate::i18n::tf(
                        "ui_retry",
                        &[
                            &attempt.to_string(),
                            &opts.retries.to_string(),
                            &active.model,
                        ],
                    )));
                    std::thread::sleep(std::time::Duration::from_millis(250 * attempt as u64));
                }
                Err((partial, was_cancelled)) => break Err((partial, was_cancelled)),
            }
        };
        let out = match out {
            Ok(o) => o,
            Err((partial, was_cancelled)) => {
                // DSH's `context-overflow` trigger: a provider-confirmed
                // overflow bypasses the pressure threshold and forces one
                // compaction, then retries the same request once.
                if !was_cancelled && is_context_overflow(&partial.finish) && opts.compact.enabled {
                    ui.on(Ev::Info(
                        crate::i18n::t("ui_context_overflow").to_string(),
                    ));
                    let mut forced = opts.compact.clone();
                    forced.threshold_tokens = 0;
                    if crate::compact::compact(cfg, &forced, msgs, opts.window, &defs, ui).is_ok() {
                        steps = steps.saturating_sub(1);
                        continue;
                    }
                }
                let text = partial.text.trim().to_string();
                if !text.is_empty() {
                    ui.on(Ev::Assistant(text.clone()));
                    msgs.push(msg("assistant", &text));
                }
                strip_attachments(msgs, user_idx, input, images);
                strip_tool_images(msgs, &mut pending_tool_images);
                report.cancelled = was_cancelled;
                report.failed = !was_cancelled;
                if !was_cancelled {
                    ui.on(Ev::Note(crate::i18n::tf(
                        "ui_request_failed",
                        &[&partial.finish],
                    )));
                    if let Some(key) = actionable_hint(&partial.finish) {
                        ui.on(Ev::Hint(crate::i18n::t(key).to_string()));
                    }
                }
                ui.on(Ev::StepEnd(steps));
                break;
            }
        };

        strip_attachments(msgs, user_idx, input, images);
        strip_tool_images(msgs, &mut pending_tool_images);

        let text = out.text.trim().to_string();
        if !text.is_empty() {
            report.text = text.clone();
            ui.on(Ev::Assistant(text.clone()));
        }

        if out.calls.is_empty() {
            if !text.is_empty() {
                msgs.push(msg("assistant", &text));
            }
            ui.on(Ev::StepEnd(steps));
            break;
        }

        msgs.push(assistant_with_tools(&text, &out.calls));

        let calls: Vec<Call> = out
            .calls
            .iter()
            .map(|c| Call {
                id: c.id.clone(),
                name: c.name.clone(),
                raw_args: c.args.clone(),
            })
            .collect();

        let outcomes = {
            let mut host = UiHost {
                ui: &mut *ui,
                api: cfg.clone(),
                sandbox: opts.sandbox.clone(),
            };
            tool::run_batch_with(reg, &calls, opts.tool_timeout_ms, &mut host)
        };

        let mut contexts: Vec<String> = Vec::new();
        for o in &outcomes {
            let idx = msgs.len();
            let mut content = J::s(&o.content);
            let mut shown_image: Option<String> = None;
            if o.name == "read_image" && !o.is_error {
                if let Some(v) = &o.value {
                    let url = v
                        .get("data_url")
                        .and_then(|x| x.as_str())
                        .filter(|u| u.starts_with("data:image/"));
                    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    if let Some(url) = url {
                        shown_image = Some(format!("[image read: {path}]\n{}", o.content));
                        content = J::Arr(vec![
                            J::o(vec![
                                ("type", J::s("text")),
                                ("text", J::s(&o.content)),
                            ]),
                            J::o(vec![
                                ("type", J::s("image_url")),
                                (
                                    "image_url",
                                    J::o(vec![("url", J::s(url))]),
                                ),
                            ]),
                        ]);
                    }
                }
            }
            msgs.push(J::o(vec![
                ("role", J::s("tool")),
                ("tool_call_id", J::s(&o.call_id)),
                ("content", content),
            ]));
            if let Some(placeholder) = shown_image {
                pending_tool_images.push((idx, placeholder));
            }
            if let Some(c) = &o.context {
                contexts.push(c.clone());
            }
            if o.name == "artifact" && !o.is_error {
                if let Some(v) = &o.value {
                    ui.on(Ev::Artifact(v.clone()));
                }
            }
        }
        if ui.interrupted() {
            // The user interrupted while the batch ran: the turn stops here.
            // The tool results stay in the history (the transcript is real),
            // and the next queued message is dispatched by the front end.
            report.cancelled = true;
            ui.on(Ev::Info(crate::i18n::t("ui_cancelled").to_string()));
            ui.on(Ev::StepEnd(steps));
            break;
        }

        // Identical-call detection (DSH thresholds 3/5/8; the first two are
        // enough on a 512 MB device). Ignore leading `state_report` calls so
        // varying progress text cannot mask a stuck tool loop.
        let tracked: Vec<&Call> = calls.iter().filter(|c| c.name != "state_report").collect();
        let tracked_slice: Vec<&Call> = if tracked.is_empty() {
            calls.iter().collect()
        } else {
            tracked
        };
        if let Some(first) = tracked_slice.first() {
            let sig = tracked_slice
                .iter()
                .map(|c| format!("{}|{}", c.name, c.raw_args))
                .collect::<Vec<_>>()
                .join("\x1f");
            if sig == repeat_sig {
                repeat_count += 1;
            } else {
                repeat_sig = sig;
                repeat_count = 1;
            }
            if repeat_count == 3 || repeat_count == 5 {
                contexts.push(repeat_reminder(&first.name, &first.raw_args, repeat_count));
            }
        }

        for c in contexts {
            ui.on(Ev::Context(c.clone()));
            msgs.push(msg("user", &c));
        }

        ui.on(Ev::StepEnd(steps));
    }

    // The ambient time message is not conversation: drop it so a restored
    // session starts with the real transcript.
    crate::context::strip_time(msgs);
    report.steps = steps;
    let fate = if report.cancelled {
        TurnFate::Cancelled
    } else if report.failed {
        TurnFate::Failed
    } else {
        TurnFate::Done
    };
    ui.on(Ev::TurnEnd { steps, fate });
    report
}


#[cfg(test)]
mod tests {
    use super::*;
    fn image_fixture() -> crate::vision::Attachment {
        crate::vision::Attachment {
            path: "/tmp/photo.png".to_string(),
            mime: "image/png",
            bytes: vec![0x89, b'P', b'N', b'G'],
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn an_image_message_uses_content_blocks() {
        let a = image_fixture();
        let m = user_msg_with_images("what is this?", &[a.clone()]);
        let content = m.get("content").and_then(|c| c.as_arr()).expect("content array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0].get("type").and_then(|t| t.as_str()), Some("text"));
        assert_eq!(
            content[0].get("text").and_then(|t| t.as_str()),
            Some("what is this?")
        );
        assert_eq!(
            content[1].get("type").and_then(|t| t.as_str()),
            Some("image_url")
        );
        let url = content[1]
            .get("image_url")
            .and_then(|i| i.get("url"))
            .and_then(|u| u.as_str())
            .expect("image url");
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        // No images: the cheap plain-string form stays.
        let plain = user_msg_with_images("hello", &[]);
        assert!(plain.get("content").and_then(|c| c.as_str()).is_some());
    }

    #[test]
    fn attachments_are_replaced_by_a_placeholder_after_the_request() {
        let a = image_fixture();
        let mut msgs = vec![msg("system", "sys"), user_msg_with_images("look", &[a])];
        strip_attachments(&mut msgs, 1, "look", &[image_fixture()]);
        let content = msgs[1]
            .get("content")
            .and_then(|c| c.as_str())
            .expect("content is text again");
        assert!(content.starts_with("look"));
        assert!(content.contains("[image attached: /tmp/photo.png"), "{content}");
        assert!(!content.contains("base64"), "payload must not stay in history");
    }

    #[test]
    fn actionable_hints_cover_common_failures() {
        assert_eq!(actionable_hint("API error: 401 unauthorized"), Some("err_hint_auth"));
        assert_eq!(actionable_hint("429 rate limit"), Some("err_hint_rate"));
        assert_eq!(actionable_hint("curl: connection timed out"), Some("err_hint_network"));
        assert_eq!(actionable_hint("stream closed unexpectedly"), Some("err_hint_network"));
        assert!(actionable_hint("bad request: invalid tool").is_none());
    }
}
