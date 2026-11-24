//! Configuration and runtime home for `dst`.
//!
//! Everything the program owns at runtime lives under `~/.dst`:
//!
//! ```text
//! ~/.dst/config.yml    all settings, including the API key (mode 600)
//! ~/.dst/sessions/     resumable conversation logs
//! ~/.dst/skills/       skill packages (SKILL.md per folder)
//! ~/.dst/mcp.json      MCP server definitions
//! ~/.dst/jobs.json     cron / file-watch jobs
//! ~/.dst/cache/        transient audio and scratch files
//! ~/.dst/logs/         voice.log and friends
//! ~/.dst/tmp/          per-run temporary files
//! ```
//!
//! ## Config dialect
//!
//! The file is YAML and follows the DeepSeek Harness patch dialect: a mapping
//! whose fields are either plain settings, or `insert:` patch rows shaped
//! `{ id, name, config, disabled }` — the same row fields DSH uses to compose
//! plugin lists. Here a row targets one dst tool by `name`:
//!
//! ```yaml
//! model: deepseek-flash
//! effort: medium
//! api:
//!   base: https://api.deepseek.com/v1
//!   key: sk-...            # kept in this same file, mode 600
//! voice:
//!   tts_voice: stella
//! tui:
//!   queued_default: true
//!   auto_send_voice: true
//!
//! # DSH-style patch rows: disable a tool, or pass it config
//! insert:
//!   - id: speak
//!     name: speak
//!     disabled: true
//!   - id: bash
//!     name: bash
//!     config:
//!       timeout_s: 30
//! ```

use std::path::PathBuf;

/// DSH-compatible YAML subset: mappings, sequences, scalars.
#[derive(Clone, Debug, PartialEq)]
pub enum Y {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Map(Vec<(String, Y)>),
    Seq(Vec<Y>),
}

impl Y {
    pub fn get(&self, key: &str) -> Option<&Y> {
        match self {
            Y::Map(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Y::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Y::Bool(b) => Some(*b),
            Y::Str(s) => match s.as_str() {
                "true" | "yes" | "on" => Some(true),
                "false" | "no" | "off" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Y::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_seq(&self) -> Option<&Vec<Y>> {
        match self {
            Y::Seq(s) => Some(s),
            _ => None,
        }
    }
}

struct Line {
    indent: usize,
    text: String,
}

fn strip_comment(s: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (None, '\'' | '"') => {
                quote = Some(c);
                out.push(c);
            }
            (Some(q), c) if c == q => {
                quote = None;
                out.push(c);
            }
            (None, '#') => break,
            _ => out.push(c),
        }
    }
    out.trim_end().to_string()
}

fn scalar(text: &str) -> Y {
    let t = text.trim();
    if t.is_empty() {
        return Y::Null;
    }
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        return Y::Str(t[1..t.len() - 1].to_string());
    }
    match t {
        "true" | "yes" | "on" => return Y::Bool(true),
        "false" | "no" | "off" => return Y::Bool(false),
        "null" | "~" => return Y::Null,
        _ => {}
    }
    if let Ok(n) = t.parse::<f64>() {
        return Y::Num(n);
    }
    Y::Str(t.to_string())
}

fn split_kv(text: &str) -> Option<(String, String)> {
    let mut quote: Option<char> = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, ':') => {
                let key = text[..i].trim().to_string();
                let val = text[i + 1..].trim().to_string();
                if key.is_empty() {
                    return None;
                }
                return Some((key, val));
            }
            _ => {}
        }
    }
    None
}

fn parse_seq(lines: &[Line], i: &mut usize) -> Y {
    let indent = lines[*i].indent;
    let mut items = Vec::new();
    while *i < lines.len() && lines[*i].indent == indent && lines[*i].text.starts_with('-') {
        let rest = lines[*i].text[1..].trim_start().to_string();
        *i += 1;
        if rest.is_empty() {
            let v = if *i < lines.len() && lines[*i].indent > indent {
                parse_block(lines, i, lines[*i].indent)
            } else {
                Y::Null
            };
            items.push(v);
            continue;
        }
        // "- key: value" starts a mapping item; following deeper lines add to it.
        if let Some((k, v)) = split_kv(&rest) {
            let mut map = Vec::new();
            map.push((k, if v.is_empty() { Y::Null } else { scalar(&v) }));
            while *i < lines.len() && lines[*i].indent > indent {
                let child_indent = lines[*i].indent;
                if let Some((ck, cv)) = split_kv(&lines[*i].text) {
                    *i += 1;
                    let val = if cv.is_empty() {
                        if *i < lines.len() && lines[*i].indent > child_indent {
                            parse_block(lines, i, lines[*i].indent)
                        } else {
                            Y::Null
                        }
                    } else {
                        scalar(&cv)
                    };
                    map.push((ck, val));
                } else {
                    *i += 1;
                }
            }
            items.push(Y::Map(map));
        } else {
            items.push(scalar(&rest));
        }
    }
    Y::Seq(items)
}

fn parse_map(lines: &[Line], i: &mut usize, indent: usize) -> Y {
    let mut map: Vec<(String, Y)> = Vec::new();
    while *i < lines.len() {
        let line_indent = lines[*i].indent;
        if line_indent < indent {
            break;
        }
        if line_indent > indent {
            *i += 1;
            continue;
        }
        if lines[*i].text.starts_with('-') {
            break;
        }
        let (key, val) = match split_kv(&lines[*i].text) {
            Some(kv) => kv,
            None => {
                *i += 1;
                continue;
            }
        };
        *i += 1;
        if val.is_empty() {
            let v = if *i < lines.len() && lines[*i].indent > indent {
                parse_block(lines, i, lines[*i].indent)
            } else {
                Y::Null
            };
            map.push((key, v));
        } else {
            map.push((key, scalar(&val)));
        }
    }
    Y::Map(map)
}

fn parse_block(lines: &[Line], i: &mut usize, indent: usize) -> Y {
    if *i < lines.len() && lines[*i].text.starts_with('-') {
        parse_seq(lines, i)
    } else {
        parse_map(lines, i, indent)
    }
}

/// Parse the YAML subset dst accepts. Never fails: malformed lines are skipped.
pub fn parse_yaml(text: &str) -> Y {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let cleaned = strip_comment(raw);
        if cleaned.trim().is_empty() {
            continue;
        }
        // Tabs are not valid YAML indentation; treat them as 2 spaces.
        let expanded = cleaned.replace('\t', "  ");
        let indent = expanded.len() - expanded.trim_start().len();
        lines.push(Line {
            indent,
            text: expanded.trim_start().to_string(),
        });
    }
    if lines.is_empty() {
        return Y::Map(Vec::new());
    }
    let mut i = 0usize;
    let indent = lines[0].indent;
    parse_block(&lines, &mut i, indent)
}

fn yaml_quote(s: &str) -> String {
    let needs = s.is_empty()
        || s.chars()
            .any(|c| matches!(c, ':' | '#' | '\'' | '"' | '\n'))
        || s.trim() != s
        || matches!(s, "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off");
    if needs {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// True when the endpoint is DeepSeek's own API. The server-side tools
/// (search/fetch/ocr) and the speech APIs only exist there.
pub fn is_official(base: &str) -> bool {
    base.trim()
        .trim_end_matches('/')
        .starts_with("https://api.deepseek.com")
}

/// Testing hook: the simulator has no microphone or speaker, and the local
/// vLLM endpoint used for offline runs has no DeepSeek speech APIs. With
/// `DST_MOCK_SPEECH=1` the voice layer fakes recording, ASR and playback so
/// the voice UI can still be exercised end to end. Never set on a device.
pub fn mock_speech() -> bool {
    matches!(
        std::env::var("DST_MOCK_SPEECH").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Transcript returned by the mocked ASR (also used by the live walkthrough).
pub fn mock_asr_text() -> String {
    std::env::var("DST_MOCK_ASR_TEXT").unwrap_or_else(|_| "turn the volume up".into())
}

pub fn home() -> PathBuf {
    let h = std::env::var("HOME").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    PathBuf::from(h).join(".dst")
}

#[allow(dead_code)] // kept for config paths that are expanded on demand
pub fn expand(path: &str) -> PathBuf {
    let p = path.trim();
    if let Some(rest) = p.strip_prefix("~/") {
        let h = std::env::var("HOME").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        return PathBuf::from(h).join(rest);
    }
    if p == "~" {
        return home();
    }
    PathBuf::from(p)
}

/// `~/...` form of a path under HOME, for one-line UI notices.
pub fn home_shorten(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

pub fn config_path() -> PathBuf {
    home().join("config.yml")
}
pub fn key_path() -> PathBuf {
    home().join("api_key")
}
pub fn jobs_path() -> PathBuf {
    home().join("jobs.json")
}
pub fn cache_dir() -> PathBuf {
    home().join("cache")
}
pub fn log_dir() -> PathBuf {
    home().join("logs")
}
pub fn tmp_dir() -> PathBuf {
    home().join("tmp")
}
pub fn sessions_dir() -> PathBuf {
    home().join("sessions")
}
pub fn skills_dir() -> PathBuf {
    home().join("skills")
}
pub fn history_path() -> PathBuf {
    home().join("history")
}

pub fn mcp_path() -> PathBuf {
    home().join("mcp.json")
}

/// Keep the newest `keep` session files and delete older ones: dst writes one
/// full-history file per launch and never removed any, so the sessions dir grew
/// with every run on a microSD card.
pub fn prune_sessions(keep: usize) {
    let mut files: Vec<(u64, std::path::PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sessions_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let mtime = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            files.push((mtime, p));
        }
    }
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    let drop = files.len() - keep;
    for (_, p) in files.into_iter().take(drop) {
        let _ = std::fs::remove_file(p);
    }
}

/// Delete stale scratch files under `~/.dst/tmp`. Artifact exports, job dumps
/// and app-test logs were written per action and never cleaned up.
pub fn prune_tmp(max_age_secs: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(rd) = std::fs::read_dir(tmp_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let old = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| now.saturating_sub(d.as_secs()) > max_age_secs)
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// Sessions ordered newest-first: (path, label, modified seconds).
pub fn list_sessions() -> Vec<(PathBuf, String, u64)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sessions_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let label = p
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("session")
                .to_string();
            let mtime = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push((p, label, mtime));
        }
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

/// Create the `~/.dst` tree with private permissions on the secret file.
fn legacy_dir() -> PathBuf {
    let h = std::env::var("HOME").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    PathBuf::from(h).join(".config").join("dsh-lite")
}

/// Move state from the pre-`~/.dst` layout on first run, without deleting the
/// old files (so a rollback stays possible).
fn migrate_legacy() {
    let l = legacy_dir();
    if !l.exists() {
        return;
    }
    if !key_path().exists() {
        if let Ok(k) = std::fs::read_to_string(l.join("api_key")) {
            if !k.trim().is_empty() {
                let _ = std::fs::write(key_path(), format!("{}\n", k.trim()));
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(key_path(), std::fs::Permissions::from_mode(0o600));
                }
            }
        }
    }
    if !jobs_path().exists() {
        if let Ok(j) = std::fs::read_to_string(l.join("jobs.json")) {
            let _ = std::fs::write(jobs_path(), j);
        }
    }
    if !config_path().exists() {
        if let Ok(txt) = std::fs::read_to_string(l.join("config.json")) {
            if let Some(v) = crate::json::parse(&txt) {
                let mut c = Config::default();
                if let Some(b) = v.get("base_url").and_then(|x| x.as_str()) {
                    c.base = b.to_string();
                }
                if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
                    c.model = m.to_string();
                }
                if let Some(m) = v.get("max_tokens").and_then(|x| x.as_num()) {
                    c.max_tokens = m as u32;
                }
                let _ = c.save();
            }
        }
    }
}

pub fn ensure_home() {
    let h = home();
    for d in [h.clone(), cache_dir(), log_dir(), tmp_dir(), sessions_dir(), skills_dir()] {
        let _ = std::fs::create_dir_all(&d);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o700));
        if let Ok(meta) = std::fs::metadata(key_path()) {
            let mut p = meta.permissions();
            if p.mode() & 0o077 != 0 {
                p.set_mode(0o600);
                let _ = std::fs::set_permissions(key_path(), p);
            }
        }
    }
}

/// What this endpoint can actually do.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub web_tools: bool,
    pub speech: bool,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub model: String,
    pub effort: String,
    pub max_tokens: u32,
    pub base: String,
    pub api_key: String,
    /// DSH account token from the real DeepSeek Platform login flow.
    /// Sent as `x-dsh-auth-token` instead of the API key.
    pub dsh_token: String,
    /// Transient-failure retries per request.
    pub api_retries: u32,
    /// Fallback models tried in order on transient failures.
    pub api_fallbacks: Vec<String>,
    pub tts_voice: String,
    pub queued_default: bool,
    pub auto_send_voice: bool,
    /// UI language: en | zh | es | ja | de | it (default en).
    pub language: String,
    /// One-time interaction tips have been shown.
    pub tips_shown: bool,
    /// First-run setup asked whether to scan the local hardware into the
    /// hardware-info skill (see hwinfo.rs). Asking happens once.
    pub hw_survey_done: bool,
    pub disabled_tools: Vec<String>,
    pub tool_config: Vec<(String, Y)>,
    /// Steps (model requests) allowed in one turn. 0 = no limit.
    pub max_steps: u32,
    /// Continuation rounds a goal may run. 0 = no limit.
    pub goal_max_rounds: u32,
    /// Steps allowed in one goal round. 0 = no limit.
    pub goal_max_steps: u32,
    /// Ask the model to judge goal completion after each round.
    pub goal_judge: bool,
    /// Automatic context compaction.
    pub compact_enabled: bool,
    /// Compact once the estimated context reaches this many tokens.
    pub compact_threshold: u64,
    /// Compaction reserves the trailing messages (aligned to a user turn).
    pub compact_keep: usize,
    /// Model used for summarisation; empty = the session model.
    pub compact_model: String,
    /// Output budget of the summarisation call.
    pub compact_max_tokens: u32,
    /// Executor deadline for tools that do not declare their own. 0 = built-in.
    pub tool_timeout_ms: u64,
    /// Inject the current date/time into the step context.
    pub context_time: bool,
    /// Minimum seconds between two injected clock readings (0 = every step).
    pub context_refresh_secs: u64,
    /// Skill packages disabled from the catalog and skill tool.
    pub disabled_skills: Vec<String>,
    /// Tool permission mode: auto | strict | full.
    pub sandbox_mode: String,
    /// Optional judge model for auto mode (empty = session model).
    pub sandbox_judge_model: String,
    pub sandbox_judge_max_tokens: u32,
    /// UI mode: normal chat, or creator (model owns and builds the device's
    /// apps, see docs/CREATOR.md).
    pub creator: bool,
    /// True once a usable API key has been provided.
    pub configured: bool,
    /// Self-update channel: `nightly` (rolling commit builds) or `stable`
    /// (newest versioned release). Default `nightly`.
    pub update_channel: String,
    /// Whale-girl desk pet. Off unless the key is set, so an old config keeps
    /// the 53-column transcript.
    pub pet_enabled: bool,
    pub pet_dock: bool,
    pub pet_room: bool,
    /// `auto` (pixel when a panel or offscreen sink exists), `pixel`, `text`.
    pub pet_render: String,
    /// Explicit framebuffer path. Empty means probe `/dev/fb_lcd` then
    /// `/dev/fb_panel`. `DST_FB` overrides this at runtime and is not saved.
    pub pet_fb: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model: "deepseek-flash".into(),
            effort: "auto".into(),
            max_tokens: 4096,
            base: "https://api.deepseek.com/v1".into(),
            api_key: String::new(),
            dsh_token: String::new(),
            api_retries: 2,
            api_fallbacks: Vec::new(),
            tts_voice: "stella".into(),
            queued_default: true,
            auto_send_voice: true,
            language: "en".into(),
            tips_shown: false,
            hw_survey_done: false,
            disabled_tools: Vec::new(),
            tool_config: Vec::new(),
            max_steps: 0,
            goal_max_rounds: 256,
            goal_max_steps: 0,
            goal_judge: true,
            compact_enabled: true,
            compact_threshold: 512_000,
            compact_keep: 2,
            compact_model: String::new(),
            compact_max_tokens: 8_192,
            tool_timeout_ms: crate::tool::DEFAULT_TIMEOUT_MS,
            context_time: true,
            context_refresh_secs: crate::context::DEFAULT_REFRESH_SECS,
            disabled_skills: Vec::new(),
            sandbox_mode: "auto".into(),
            sandbox_judge_model: String::new(),
            sandbox_judge_max_tokens: 48,
            creator: false,
            configured: false,
            update_channel: "nightly".into(),
            pet_enabled: false,
            pet_dock: true,
            pet_room: true,
            pet_render: "auto".into(),
            pet_fb: String::new(),
        }
    }
}

impl Config {
    /// Tools and features that require the official endpoint.
    pub fn capabilities(&self) -> Capabilities {
        let official = is_official(&self.base);
        Capabilities {
            web_tools: official,
            // `DST_MOCK_SPEECH` lets the simulator (and the live walkthrough
            // against a local vLLM endpoint) exercise the voice UI without a
            // microphone, speaker or DeepSeek speech API.
            speech: official || mock_speech(),
        }
    }

    pub fn load() -> Config {
        ensure_home();
        migrate_legacy();
        let mut cfg = Config::default();
        // System layer first, then the user file on top (DSH composes layers
        // in order and lets the later layer win).
        for path in [PathBuf::from("/etc/dst/config.yml"), config_path()] {
            if let Ok(text) = std::fs::read_to_string(&path) {
                cfg.apply(&parse_yaml(&text));
            }
        }
        // Legacy layouts are read once, then rewritten inline by a later save.
        if cfg.api_key.is_empty() {
            if let Ok(k) = std::fs::read_to_string(key_path()) {
                cfg.api_key = k.trim().to_string();
            }
        }
        if cfg.api_key.is_empty() {
            if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
                cfg.api_key = k.trim().to_string();
            }
        }
        // An explicit api.key in the config wins over the key file.
        if cfg.api_key.is_empty() {
            if let Ok(text) = std::fs::read_to_string(config_path()) {
                if let Some(Y::Str(k)) = parse_yaml(&text).get("api").and_then(|a| a.get("key")) {
                    cfg.api_key = k.trim().to_string();
                }
            }
        }
        cfg.configured = !cfg.api_key.is_empty() || !cfg.dsh_token.is_empty();
        cfg
    }

    fn apply(&mut self, doc: &Y) {
        if let Some(v) = doc.get("model").and_then(|v| v.as_str()) {
            self.model = v.to_string();
        }
        if let Some(v) = doc.get("effort").and_then(|v| v.as_str()) {
            self.effort = v.to_string();
        }
        if let Some(v) = doc.get("max_tokens").and_then(|v| v.as_f64()) {
            self.max_tokens = v as u32;
        }
        if let Some(api) = doc.get("api") {
            if let Some(v) = api.get("base").and_then(|v| v.as_str()) {
                self.base = v.to_string();
            }
            if let Some(v) = api.get("key").and_then(|v| v.as_str()) {
                self.api_key = v.to_string();
            }
            if let Some(v) = api.get("dsh_token").and_then(|v| v.as_str()) {
                self.dsh_token = v.to_string();
            }
            if let Some(v) = api.get("retries").and_then(|v| v.as_f64()) {
                self.api_retries = v.max(0.0) as u32;
            }
            if let Some(seq) = api.get("fallback_models").and_then(|v| v.as_seq()) {
                self.api_fallbacks = seq.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
            }
        }
        if let Some(tui) = doc.get("tui") {
            if let Some(v) = tui.get("queued_default").and_then(|v| v.as_bool()) {
                self.queued_default = v;
            }
            if let Some(v) = tui.get("auto_send_voice").and_then(|v| v.as_bool()) {
                self.auto_send_voice = v;
            }
            if let Some(v) = tui.get("language").and_then(|v| v.as_str()) {
                self.language = v.to_string();
            }
            if let Some(v) = tui.get("tips_shown").and_then(|v| v.as_bool()) {
                self.tips_shown = v;
            }
            if let Some(v) = tui.get("hw_survey_done").and_then(|v| v.as_bool()) {
                self.hw_survey_done = v;
            }
            if let Some(v) = tui.get("mode").and_then(|v| v.as_str()) {
                self.creator = v.eq_ignore_ascii_case("creator");
            }
        }
        if let Some(update) = doc.get("update") {
            if let Some(v) = update.get("channel").and_then(|v| v.as_str()) {
                if crate::update::Channel::parse(v).is_some() {
                    self.update_channel = v.trim().to_ascii_lowercase();
                }
            }
        }
        // Tolerate the flat spelling too (hand-written configs).
        if let Some(v) = doc.get("update_channel").and_then(|v| v.as_str()) {
            if crate::update::Channel::parse(v).is_some() {
                self.update_channel = v.trim().to_ascii_lowercase();
            }
        }
        if let Some(voice) = doc.get("voice") {
            if let Some(v) = voice.get("tts_voice").and_then(|v| v.as_str()) {
                self.tts_voice = v.to_string();
            }
        }
        if let Some(agent) = doc.get("agent") {
            if let Some(v) = agent.get("max_steps").and_then(|v| v.as_f64()) {
                self.max_steps = v.max(0.0) as u32;
            }
        }
        if let Some(goal) = doc.get("goal") {
            if let Some(v) = goal.get("max_rounds").and_then(|v| v.as_f64()) {
                self.goal_max_rounds = v.max(0.0) as u32;
            }
            if let Some(v) = goal.get("max_steps").and_then(|v| v.as_f64()) {
                self.goal_max_steps = v.max(0.0) as u32;
            }
            if let Some(v) = goal.get("judge").and_then(|v| v.as_bool()) {
                self.goal_judge = v;
            }
        }
        if let Some(c) = doc.get("compaction") {
            if let Some(v) = c.get("enabled").and_then(|v| v.as_bool()) {
                self.compact_enabled = v;
            }
            if let Some(v) = c.get("threshold_tokens").and_then(|v| v.as_f64()) {
                self.compact_threshold = v.max(0.0) as u64;
            }
            if let Some(v) = c.get("keep_messages").and_then(|v| v.as_f64()) {
                self.compact_keep = v.max(0.0) as usize;
            }
            if let Some(v) = c.get("model").and_then(|v| v.as_str()) {
                self.compact_model = v.to_string();
            }
            if let Some(v) = c.get("max_tokens").and_then(|v| v.as_f64()) {
                self.compact_max_tokens = v.max(0.0) as u32;
            }
        }
        if let Some(tools) = doc.get("tools") {
            if let Some(v) = tools.get("call_timeout_ms").and_then(|v| v.as_f64()) {
                self.tool_timeout_ms = v.max(0.0) as u64;
            }
        }
        if let Some(sk) = doc.get("skills") {
            if let Some(seq) = sk.get("disabled").and_then(|v| v.as_seq()) {
                self.disabled_skills = seq
                    .iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }
        if let Some(sb) = doc.get("sandbox") {
            if let Some(v) = sb.get("mode").and_then(|v| v.as_str()) {
                self.sandbox_mode = v.to_string();
            }
            if let Some(v) = sb.get("judge_model").and_then(|v| v.as_str()) {
                self.sandbox_judge_model = v.to_string();
            }
            if let Some(v) = sb.get("judge_max_tokens").and_then(|v| v.as_f64()) {
                self.sandbox_judge_max_tokens = v.max(8.0) as u32;
            }
        }
        if let Some(pet) = doc.get("pet") {
            if let Some(v) = pet.get("enabled").and_then(|v| v.as_bool()) {
                self.pet_enabled = v;
            }
            if let Some(v) = pet.get("dock").and_then(|v| v.as_bool()) {
                self.pet_dock = v;
            }
            if let Some(v) = pet.get("room").and_then(|v| v.as_bool()) {
                self.pet_room = v;
            }
            if let Some(v) = pet.get("render").and_then(|v| v.as_str()) {
                if matches!(v, "auto" | "pixel" | "text") {
                    self.pet_render = v.to_string();
                }
            }
            if let Some(v) = pet.get("fb").and_then(|v| v.as_str()) {
                self.pet_fb = v.to_string();
            }
        }
        if let Some(ctx) = doc.get("context") {
            if let Some(v) = ctx.get("time").and_then(|v| v.as_bool()) {
                self.context_time = v;
            }
            if let Some(v) = ctx.get("time_refresh_secs").and_then(|v| v.as_f64()) {
                self.context_refresh_secs = v.max(0.0) as u64;
            }
        }
        // DSH patch dialect: insert rows of { id, name, config, disabled }.
        if let Some(rows) = doc.get("insert").and_then(|v| v.as_seq()) {
            for row in rows {
                let name = row
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| row.get("id").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                if row.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                    self.disabled_tools.push(name.to_string());
                }
                if let Some(c) = row.get("config") {
                    self.tool_config.push((name.to_string(), c.clone()));
                }
            }
        }
    }

    /// The effective document written back to `~/.dst/config.yml`.
    pub fn to_yaml(&self) -> String {
        let mut s = String::new();
        s.push_str("# dst configuration — syntax follows the DeepSeek Harness\n");
        s.push_str("# patch dialect: plain keys, plus `insert:` rows { id, name, config, disabled }.\n");
        s.push_str(&format!("model: {}\n", yaml_quote(&self.model)));
        s.push_str(&format!("effort: {}\n", yaml_quote(&self.effort)));
        s.push_str(&format!("max_tokens: {}\n", self.max_tokens));
        s.push_str("api:\n");
        s.push_str(&format!("  base: {}\n", yaml_quote(&self.base)));
        if !self.api_fallbacks.is_empty() {
            s.push_str("  fallback_models:\n");
            for m in &self.api_fallbacks {
                s.push_str(&format!("    - {}\n", yaml_quote(m)));
            }
        }
        s.push_str(&format!("  retries: {}\n", self.api_retries));
        if !self.dsh_token.is_empty() {
            s.push_str(&format!("  dsh_token: {}\n", yaml_quote(&self.dsh_token.trim())));
        }
        if !self.api_key.is_empty() {
            s.push_str(&format!("  key: {}\n", yaml_quote(&self.api_key.trim())));
        }
        s.push_str("update:\n");
        s.push_str(&format!(
            "  channel: {}\n",
            crate::update::Channel::parse(&self.update_channel)
                .unwrap_or(crate::update::Channel::Nightly)
                .as_str()
        ));
        s.push_str("voice:\n");
        s.push_str(&format!("  tts_voice: {}\n", yaml_quote(&self.tts_voice)));
        s.push_str("tui:\n");
        s.push_str(&format!("  queued_default: {}\n", self.queued_default));
        s.push_str(&format!("  auto_send_voice: {}\n", self.auto_send_voice));
        s.push_str(&format!("  language: {}\n", yaml_quote(&self.language)));
        s.push_str(&format!("  tips_shown: {}\n", self.tips_shown));
        s.push_str(&format!("  hw_survey_done: {}\n", self.hw_survey_done));
        s.push_str(&format!(
            "  mode: {}\n",
            if self.creator { "creator" } else { "normal" }
        ));
        s.push_str("agent:\n");
        s.push_str(&format!("  max_steps: {}\n", self.max_steps));
        s.push_str("goal:\n");
        s.push_str(&format!("  max_rounds: {}\n", self.goal_max_rounds));
        s.push_str(&format!("  max_steps: {}\n", self.goal_max_steps));
        s.push_str(&format!("  judge: {}\n", self.goal_judge));
        s.push_str("compaction:\n");
        s.push_str(&format!("  enabled: {}\n", self.compact_enabled));
        s.push_str(&format!("  threshold_tokens: {}\n", self.compact_threshold));
        s.push_str(&format!("  keep_messages: {}\n", self.compact_keep));
        s.push_str(&format!("  max_tokens: {}\n", self.compact_max_tokens));
        if !self.compact_model.is_empty() {
            s.push_str(&format!("  model: {}\n", yaml_quote(&self.compact_model)));
        }
        s.push_str("tools:\n");
        s.push_str(&format!("  call_timeout_ms: {}\n", self.tool_timeout_ms));
        if !self.disabled_skills.is_empty() {
            s.push_str("skills:\n  disabled:\n");
            for name in &self.disabled_skills {
                s.push_str(&format!("    - {}\n", yaml_quote(name)));
            }
        }
        s.push_str("sandbox:\n");
        s.push_str(&format!("  mode: {}\n", yaml_quote(&self.sandbox_mode)));
        if !self.sandbox_judge_model.is_empty() {
            s.push_str(&format!("  judge_model: {}\n", yaml_quote(&self.sandbox_judge_model)));
        }
        s.push_str(&format!("  judge_max_tokens: {}\n", self.sandbox_judge_max_tokens));
        s.push_str("context:\n");
        s.push_str(&format!("  time: {}\n", self.context_time));
        s.push_str(&format!("  time_refresh_secs: {}\n", self.context_refresh_secs));
        s.push_str("pet:\n");
        s.push_str(&format!("  enabled: {}\n", self.pet_enabled));
        s.push_str(&format!("  dock: {}\n", self.pet_dock));
        s.push_str(&format!("  room: {}\n", self.pet_room));
        s.push_str(&format!("  render: {}\n", yaml_quote(&self.pet_render)));
        if !self.pet_fb.is_empty() {
            s.push_str(&format!("  fb: {}\n", yaml_quote(&self.pet_fb)));
        }
        if !self.disabled_tools.is_empty() || !self.tool_config.is_empty() {
            s.push_str("insert:\n");
            for t in &self.disabled_tools {
                s.push_str(&format!("  - id: {t}\n    name: {t}\n    disabled: true\n"));
            }
            for (name, _c) in &self.tool_config {
                s.push_str(&format!("  - id: {name}\n    name: {name}\n"));
            }
        }
        s
    }

    /// Persist config (and the key, separately, mode 600).
    pub fn save(&self) -> Result<(), String> {
        ensure_home();
        let p = config_path();
        std::fs::write(&p, self.to_yaml()).map_err(|e| format!("{}: {e}", p.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        // The key now lives in config.yml; remove the old side-car file.
        let _ = std::fs::remove_file(key_path());
        Ok(())
    }

    /// One-line summary for `/config`.
    pub fn summary(&self) -> Vec<String> {
        let shown = if !self.dsh_token.is_empty() {
            "已配置 (DSH token)".to_string()
        } else if self.api_key.is_empty() {
            "未配置".to_string()
        } else {
            "已配置".to_string()
        };
        vec![
            format!("model      {}", self.model),
            format!("effort     {}", self.effort),
            format!("base       {}", self.base),
            format!("api key    {shown}"),
            format!("max_tokens {}", self.max_tokens),
            format!("tts_voice  {}", self.tts_voice),
            format!(
                "queued     {}   auto_send_voice {}   language {}",
                self.queued_default, self.auto_send_voice, self.language
            ),
            format!(
                "max_steps  {}",
                if self.max_steps == 0 { "unlimited".to_string() } else { self.max_steps.to_string() }
            ),
            format!(
                "goal       rounds {}   steps/round {}   judge {}",
                if self.goal_max_rounds == 0 { "unlimited".to_string() } else { self.goal_max_rounds.to_string() },
                if self.goal_max_steps == 0 { "unlimited".to_string() } else { self.goal_max_steps.to_string() },
                self.goal_judge
            ),
            format!(
                "compact    {}   threshold {} tokens   keep {}",
                if self.compact_enabled { "on" } else { "off" },
                crate::compact::human_tokens(self.compact_threshold),
                self.compact_keep
            ),
            format!(
                "retries    {}   fallbacks {}",
                self.api_retries,
                if self.api_fallbacks.is_empty() { "-".to_string() } else { self.api_fallbacks.join(",") }
            ),
            format!(
                "tool call  timeout {}ms   time-context {}",
                self.tool_timeout_ms, self.context_time
            ),
            format!(
                "sandbox    {}{}",
                self.sandbox_mode,
                if self.sandbox_judge_model.is_empty() {
                    String::new()
                } else {
                    format!("   judge {}", self.sandbox_judge_model)
                }
            ),
            format!("home       {}", home().display()),
            format!("config     {}", config_path().display()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_and_patches() {
        let y = parse_yaml(
            "model: deepseek-flash\n\
             api:\n  base: https://x/v1\n  key_file: ~/.dst/api_key\n\
             tui:\n  queued_default: false\n\
             insert:\n  - id: speak\n    name: speak\n    disabled: true\n",
        );
        assert_eq!(y.get("model").unwrap().as_str(), Some("deepseek-flash"));
        assert_eq!(
            y.get("api").unwrap().get("base").unwrap().as_str(),
            Some("https://x/v1")
        );
        assert_eq!(
            y.get("tui").unwrap().get("queued_default").unwrap().as_bool(),
            Some(false)
        );
        let rows = y.get("insert").unwrap().as_seq().unwrap();
        assert_eq!(rows[0].get("name").unwrap().as_str(), Some("speak"));
        assert_eq!(rows[0].get("disabled").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn roundtrip_yaml() {
        let mut c = Config::default();
        c.creator = true;
        c.model = "deepseek-v4".into();
        c.disabled_tools.push("speak".into());
        let doc = parse_yaml(&c.to_yaml());
        assert_eq!(doc.get("model").unwrap().as_str(), Some("deepseek-v4"));
        assert_eq!(
            doc.get("tui").unwrap().get("mode").unwrap().as_str(),
            Some("creator"),
            "the UI mode persists"
        );
        let rows = doc.get("insert").unwrap().as_seq().unwrap();
        assert_eq!(rows[0].get("disabled").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn legacy_yaml_is_parseable() {
        // The shape written by earlier dst versions must still load.
        let y = parse_yaml(
            "insert:\n  - id: bash\n    name: bash\n\n  - id: speak\n    name: speak\n",
        );
        assert_eq!(y.get("insert").unwrap().as_seq().unwrap().len(), 2);
    }

    #[test]
    fn parses_agent_goal_compaction_context() {
        let y = parse_yaml(
            "agent:\n  max_steps: 20\n\
             goal:\n  max_rounds: 5\n  max_steps: 30\n  judge: false\n\
             compaction:\n  enabled: false\n  threshold_tokens: 128000\n  keep_messages: 3\n  max_tokens: 4096\n\
             tools:\n  call_timeout_ms: 9000\n\
             context:\n  time: false\n  time_refresh_secs: 0\n",
        );
        let mut c = Config::default();
        c.apply(&y);
        assert_eq!(c.max_steps, 20);
        assert_eq!(c.goal_max_rounds, 5);
        assert_eq!(c.goal_max_steps, 30);
        assert!(!c.goal_judge);
        assert!(!c.compact_enabled);
        assert_eq!(c.compact_threshold, 128_000);
        assert_eq!(c.compact_keep, 3);
        assert_eq!(c.compact_max_tokens, 4_096);
        assert_eq!(c.tool_timeout_ms, 9_000);
        assert!(!c.context_time);
        assert_eq!(c.context_refresh_secs, 0);
        // And the written form loads back with the same values.
        let doc = parse_yaml(&c.to_yaml());
        let mut c2 = Config::default();
        c2.apply(&doc);
        assert_eq!(c2.max_steps, 20);
        assert_eq!(c2.compact_threshold, 128_000);
        assert_eq!(c2.goal_max_rounds, 5);
    }

    #[test]
    fn comments_and_quotes() {
        let y = parse_yaml("a: \"x # not comment\" # real comment\nb: 'y'\n");
        assert_eq!(y.get("a").unwrap().as_str(), Some("x # not comment"));
        assert_eq!(y.get("b").unwrap().as_str(), Some("y"));
    }
}
