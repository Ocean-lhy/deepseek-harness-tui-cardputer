//! Growth, the 45 achievement ids, and the `dsh-whale-musume` save format.
//!
//! The numeric rules and the `WM1-` / JSON schema follow
//! `assets/whale-moe-core.js` in Sutera-Diffusus/dsh-whale-musume (MIT) so a
//! save can move in either direction. Only the rules are reimplemented; no
//! file from that repository is copied. Device-only fields (`last_seen`) are
//! never written into an export.

use crate::json::J;

pub const SCHEMA: &str = "dsh-whale-musume/save";
pub const VERSION: i64 = 1;
pub const MOOD_MAX: i32 = 100;
pub const AFFINITY_MAX: i32 = 10_000;
pub const SATIETY_MAX: i32 = 100;
pub const LEVEL_STEP: i32 = 500;
const MAX_TEXT: usize = 200_000;
const MAX_JSON: usize = 65_536;

/// Ids the reference catalog accepts. Unknown ids are dropped on import,
/// which is the reference behaviour. The first-version device can *trigger*
/// the subset named in `device_triggers`; the rest stay listed so an imported
/// wall still shows them.
pub const ACHIEVEMENTS: &[(&str, &str)] = &[
    ("first-pat", "初次摸头"),
    ("ten-pats", "摸头十连"),
    ("hundred-pats", "摸头百连"),
    ("first-feed", "投喂成功"),
    ("first-triple", "三连击"),
    ("thanks", "嘴甜"),
    ("lv5", "五级"),
    ("lv10", "十级"),
    ("signin3", "常客"),
    ("signin7", "一周之约"),
    ("night-owl", "深夜陪伴"),
    ("comeback", "欢迎回来"),
    ("day1", "一日之缘"),
    ("day7", "一周相伴"),
    ("day30", "三十日契约"),
    ("day100", "百日之约"),
    ("day365", "一年之约"),
    ("first-tool", "开工啦"),
    ("tools-10", "工具十连"),
    ("tools-50", "工具五十连"),
    ("tools-100", "工具百连"),
    ("first-code", "代码初体验"),
    ("code-20", "代码狂人"),
    ("first-success", "旗开得胜"),
    ("success-10", "任务十连"),
    ("first-failure", "初次翻车"),
    ("fail-10", "翻车十连"),
    ("messages-100", "会话百条"),
    ("messages-500", "消息五百条"),
    ("keyword-master", "关键词大师"),
    ("night-work", "深夜赶工"),
    ("balance-low", "余额告急"),
    ("game-first", "初次开玩"),
    ("game-win", "泡泡之王"),
    ("game-combo10", "连击达人"),
    ("game-highscore", "纪录刷新"),
    ("memory-first", "翻牌初体验"),
    ("memory-clear", "全记住了"),
    ("memory-perfect", "一次不翻错"),
    ("memory-combo6", "记忆连击"),
    ("quest-first", "任务初体验"),
    ("quest-all", "一日全勤"),
    ("week-signin7", "周常满勤"),
    ("bond-action", "新动作解锁"),
    ("bond-badge", "称号首解锁"),
];

/// Achievements this device can actually unlock. The others are imported and
/// shown, and are not granted here (no chat scanning, balance, or minigames).
pub fn device_can_trigger(id: &str) -> bool {
    !matches!(
        id,
        "thanks"
            | "keyword-master"
            | "balance-low"
            | "game-first"
            | "game-win"
            | "game-combo10"
            | "game-highscore"
            | "memory-first"
            | "memory-clear"
            | "memory-perfect"
            | "memory-combo6"
            | "quest-first"
            | "quest-all"
            | "week-signin7"
            | "bond-action"
            | "bond-badge"
    )
}

#[derive(Clone, Debug)]
pub struct Growth {
    pub mood: i32,
    pub affinity: i32,
    pub satiety: f64,
    pub level: i32,
    pub last_signin: String,
    pub signin_streak: i32,
    pub achievements: Vec<String>,
}

impl Default for Growth {
    fn default() -> Self {
        Self {
            mood: 70,
            affinity: 0,
            satiety: 80.0,
            level: 1,
            last_signin: String::new(),
            signin_streak: 0,
            achievements: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub tools: u32,
    pub code: u32,
    pub success: u32,
    pub failure: u32,
    pub messages: u32,
    pub pats: u32,
    pub feeds: u32,
    pub triples: u32,
}

#[derive(Clone, Debug)]
pub struct Save {
    pub growth: Growth,
    pub usage: Usage,
    pub title: String,
    pub self_name: String,
    pub badge: String,
    pub companion_since: i64,
    pub last_seen: i64,
    /// Raw JSON values preserved so a round trip does not drop quests, the
    /// weekly board, minigame stats or the diary.
    pub quests: String,
    pub week: String,
    pub game: String,
    pub journal: String,
}

impl Default for Save {
    fn default() -> Self {
        let mut s = Self {
            growth: Growth::default(),
            usage: Usage::default(),
            title: String::new(),
            self_name: String::new(),
            badge: String::new(),
            companion_since: 0,
            last_seen: 0,
            quests: String::new(),
            week: String::new(),
            game: String::new(),
            journal: String::new(),
        };
        s.relevel();
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GEvent {
    Pat,
    Feed,
    Praise,
    Triple,
    Success,
    Failure,
    Signin,
    /// Minutes elapsed, for the satiety decay (0.15 per minute).
    Tick { mins: f64 },
}

impl Save {
    pub fn relevel(&mut self) {
        self.growth.level = (self.growth.affinity / LEVEL_STEP + 1).max(1);
    }

    pub fn mood_pct(&self) -> u8 {
        self.growth.mood.clamp(0, 100) as u8
    }
    pub fn satiety_pct(&self) -> u8 {
        self.growth.satiety.clamp(0.0, 100.0) as u8
    }
    /// Bond bar fills once per level step, so it moves before the level does.
    pub fn bond_pct(&self) -> u8 {
        (self.growth.affinity % LEVEL_STEP * 100 / LEVEL_STEP).clamp(0, 100) as u8
    }

    pub fn apply(&mut self, ev: GEvent, now_ms: i64) -> Vec<String> {
        let g = &mut self.growth;
        match ev {
            GEvent::Pat => { g.mood += 4; g.affinity += 2; self.usage.pats += 1; }
            GEvent::Feed => { g.satiety += 30.0; g.affinity += 5; g.mood += 3; self.usage.feeds += 1; }
            GEvent::Praise => { g.mood += 5; g.affinity += 8; }
            GEvent::Triple => { g.mood += 10; g.affinity += 10; self.usage.triples += 1; }
            GEvent::Success => { g.mood += 3; self.usage.success += 1; }
            GEvent::Failure => { g.mood -= 5; self.usage.failure += 1; }
            GEvent::Tick { mins } => { g.satiety -= mins * 0.15; }
            GEvent::Signin => {
                let today = local_day(now_ms);
                if g.last_signin != today {
                    let yday = local_day(now_ms - 86_400_000);
                    g.signin_streak = if g.last_signin == yday { g.signin_streak + 1 } else { 1 };
                    g.last_signin = today;
                    g.mood += 5;
                }
            }
        }
        g.mood = g.mood.clamp(0, MOOD_MAX);
        g.affinity = g.affinity.clamp(0, AFFINITY_MAX);
        g.satiety = g.satiety.clamp(0.0, SATIETY_MAX as f64);
        self.relevel();
        self.grant(now_ms)
    }

    pub fn note_tool(&mut self, kind_is_code: bool, now_ms: i64) -> Vec<String> {
        self.usage.tools += 1;
        if kind_is_code {
            self.usage.code += 1;
        }
        self.grant(now_ms)
    }

    pub fn note_message(&mut self, now_ms: i64) -> Vec<String> {
        self.usage.messages += 1;
        self.grant(now_ms)
    }

    fn grant(&mut self, now_ms: i64) -> Vec<String> {
        let mut add = Vec::new();
        let have = &self.growth.achievements;
        let push = |id: &str, add: &mut Vec<String>, have: &[String]| {
            if device_can_trigger(id) && !have.iter().any(|h| h == id) && !add.iter().any(|h| h == id) {
                add.push(id.to_string());
            }
        };
        let pats = self.usage.pats;
        if pats >= 1 { push("first-pat", &mut add, have); }
        if pats >= 10 { push("ten-pats", &mut add, have); }
        if pats >= 100 { push("hundred-pats", &mut add, have); }
        if self.usage.feeds >= 1 { push("first-feed", &mut add, have); }
        if self.usage.triples >= 1 { push("first-triple", &mut add, have); }
        if self.growth.level >= 5 { push("lv5", &mut add, have); }
        if self.growth.level >= 10 { push("lv10", &mut add, have); }
        if self.growth.signin_streak >= 3 { push("signin3", &mut add, have); }
        if self.growth.signin_streak >= 7 { push("signin7", &mut add, have); }
        let u = &self.usage;
        if u.tools >= 1 { push("first-tool", &mut add, have); }
        if u.tools >= 10 { push("tools-10", &mut add, have); }
        if u.tools >= 50 { push("tools-50", &mut add, have); }
        if u.tools >= 100 { push("tools-100", &mut add, have); }
        if u.code >= 1 { push("first-code", &mut add, have); }
        if u.code >= 20 { push("code-20", &mut add, have); }
        if u.success >= 1 { push("first-success", &mut add, have); }
        if u.success >= 10 { push("success-10", &mut add, have); }
        if u.failure >= 1 { push("first-failure", &mut add, have); }
        if u.failure >= 10 { push("fail-10", &mut add, have); }
        if u.messages >= 100 { push("messages-100", &mut add, have); }
        if u.messages >= 500 { push("messages-500", &mut add, have); }
        if self.companion_since > 0 {
            let days = ((now_ms - self.companion_since) / 86_400_000).max(0);
            for (need, id) in [(1, "day1"), (7, "day7"), (30, "day30"), (100, "day100"), (365, "day365")] {
                if days >= need { push(id, &mut add, have); }
            }
        }
        self.growth.achievements.extend(add.iter().cloned());
        add
    }

    pub fn grant_id(&mut self, id: &str) -> bool {
        if !device_can_trigger(id) || self.growth.achievements.iter().any(|h| h == id) {
            return false;
        }
        if !ACHIEVEMENTS.iter().any(|(k, _)| *k == id) {
            return false;
        }
        self.growth.achievements.push(id.to_string());
        true
    }

    pub fn remember(&mut self, now_ms: i64, kind: &str, text: &str) {
        let mut arr = match crate::json::parse(&self.journal) {
            Some(J::Arr(a)) => a,
            _ => Vec::new(),
        };
        arr.push(J::o(vec![
            ("at", J::Num(now_ms as f64)),
            ("kind", J::s(kind)),
            ("text", J::s(text)),
        ]));
        if arr.len() > 80 {
            arr.drain(0..arr.len() - 80);
        }
        self.journal = J::Arr(arr).dump();
    }
}

pub fn achievement_name(id: &str) -> &str {
    ACHIEVEMENTS.iter().find(|(k, _)| *k == id).map(|(_, n)| *n).unwrap_or(id)
}

/// Local calendar day as `YYYY-M-D` (no zero padding), matching the reference.
pub fn local_day(ms: i64) -> String {
    let (y, m, d, _) = civil(ms, true);
    format!("{y}-{m}-{d}")
}

pub fn local_hour(ms: i64) -> i32 {
    civil(ms, true).3
}

pub fn night_hour(hour: i32) -> bool {
    !(6..22).contains(&hour)
}

fn civil(ms: i64, local: bool) -> (i32, i32, i32, i32) {
    let secs = ms.div_euclid(1000);
    let mut tm = Tm::zero();
    let ptr = if local {
        unsafe { localtime_r(&secs, &mut tm) }
    } else {
        unsafe { gmtime_r(&secs, &mut tm) }
    };
    if ptr.is_null() {
        return (1970, 1, 1, 0);
    }
    (tm.year + 1900, tm.mon + 1, tm.mday, tm.hour)
}

pub fn iso_utc(ms: i64) -> String {
    let (y, m, d, h) = civil(ms, false);
    let secs = ms.div_euclid(1000);
    let mut tm = Tm::zero();
    unsafe { gmtime_r(&secs, &mut tm); }
    let millis = ms.rem_euclid(1000);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{:02}:{:02}.{millis:03}Z", tm.min, tm.sec)
}

#[repr(C)]
struct Tm {
    sec: i32, min: i32, hour: i32, mday: i32, mon: i32, year: i32,
    wday: i32, yday: i32, isdst: i32,
    gmtoff: i64, zone: *const i8,
}

impl Tm {
    fn zero() -> Self {
        Self {
            sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0,
            wday: 0, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null(),
        }
    }
}

extern "C" {
    fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
    fn gmtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
}

// ---------------------------------------------------------------- portable

pub fn export_json(save: &Save, now_ms: i64, app_version: &str) -> String {
    let data = data_object(save, false);
    J::o(vec![
        ("schema", J::s(SCHEMA)),
        ("version", J::Num(VERSION as f64)),
        ("exportedAt", J::s(&iso_utc(now_ms))),
        ("appVersion", J::s(&clip(app_version, 32))),
        ("data", data),
    ]).dump()
}

pub fn export_code(save: &Save, now_ms: i64, app_version: &str) -> String {
    let data = data_object(save, true);
    let json = J::o(vec![
        ("schema", J::s(SCHEMA)),
        ("version", J::Num(VERSION as f64)),
        ("exportedAt", J::s(&iso_utc(now_ms))),
        ("appVersion", J::s(&clip(app_version, 32))),
        ("data", data),
    ]).dump();
    let sum = checksum(&json);
    let b64 = base64_encode(&json).replace('+', "-").replace('/', "_").trim_end_matches('=').to_string();
    format!("WM1-{sum}-{b64}")
}

fn data_object(save: &Save, core_only: bool) -> J {
    let g = &save.growth;
    let mut pairs = vec![
        ("mood", J::s(&g.mood.to_string())),
        ("affinity", J::s(&g.affinity.to_string())),
        ("satiety", J::s(&((g.satiety.round() as i32).clamp(0, SATIETY_MAX)).to_string())),
        ("lastSignin", J::s(&g.last_signin)),
        ("signinStreak", J::s(&g.signin_streak.to_string())),
        ("achievements", J::s(&clean_achievements(&g.achievements.join(",")).join(","))),
        ("companionSince", J::s(&if save.companion_since > 0 { save.companion_since.to_string() } else { String::new() })),
        ("level", J::s(&g.level.to_string())),
    ];
    if core_only {
        if !save.journal.is_empty() { pairs.push(("journal", J::s(&save.journal))); }
        pairs.push(("badge", J::s(&clip(&save.badge, 40))));
    } else {
        if !save.quests.is_empty() { pairs.push(("quests", J::s(&save.quests))); }
        if !save.week.is_empty() { pairs.push(("weekSignin", J::s(&save.week))); }
        pairs.push(("badge", J::s(&clip(&save.badge, 40))));
        if !save.game.is_empty() { pairs.push(("gameStats", J::s(&save.game))); }
        let usage = J::o(vec![
            ("tools", J::Num(save.usage.tools as f64)),
            ("code", J::Num(save.usage.code as f64)),
            ("success", J::Num(save.usage.success as f64)),
            ("failure", J::Num(save.usage.failure as f64)),
            ("messages", J::Num(save.usage.messages as f64)),
            ("pats", J::Num(save.usage.pats as f64)),
            ("feeds", J::Num(save.usage.feeds as f64)),
            ("triples", J::Num(save.usage.triples as f64)),
        ]).dump();
        pairs.push(("usageStats", J::s(&usage)));
        if !save.journal.is_empty() { pairs.push(("journal", J::s(&save.journal))); }
    }
    pairs.push(("title", J::s(&clip(&save.title, 20))));
    pairs.push(("selfName", J::s(&clip(&save.self_name, 20))));
    J::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn import_text(text: &str, now_ms: i64) -> Result<Save, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("内容是空的".into());
    }
    if trimmed.len() > MAX_TEXT {
        return Err("内容太大，不像是存档".into());
    }
    if trimmed.starts_with("WM1-") {
        import_code(trimmed, now_ms)
    } else {
        import_json(trimmed, now_ms)
    }
}

fn import_json(text: &str, now_ms: i64) -> Result<Save, String> {
    let parsed = crate::json::parse(text).ok_or("不是合法的 JSON")?;
    if parsed.get("schema").and_then(|v| v.as_str()) != Some(SCHEMA) {
        return Err("这不是鲸鱼娘的存档文件".into());
    }
    let version = parsed.get("version").and_then(|v| v.as_num()).unwrap_or(0.0) as i64;
    if !(1..=VERSION).contains(&version) {
        return Err(format!("存档版本不兼容（当前支持 v{VERSION}）"));
    }
    let data = parsed.get("data").ok_or("存档里没有可用的数据")?;
    if !matches!(data, J::Obj(_)) {
        return Err("存档里没有可用的数据".into());
    }
    let save = sanitize(data, now_ms)?;
    Ok(save)
}

fn import_code(text: &str, now_ms: i64) -> Result<Save, String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    // The payload is base64url, so it may itself contain '-'. Split only twice.
    let Some(rest) = compact.strip_prefix("WM1-") else {
        return Err("迁移码格式不对（应以 WM1- 开头）".into());
    };
    let Some((sum, payload)) = rest.split_once('-') else {
        return Err("迁移码格式不对（应以 WM1- 开头）".into());
    };
    if sum.len() != 8 || payload.is_empty() || !payload.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("迁移码格式不对（应以 WM1- 开头）".into());
    }
    if !sum.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("迁移码格式不对（应以 WM1- 开头）".into());
    }
    let json = base64_decode(&payload.replace('-', "+").replace('_', "/")).map_err(|_| "迁移码解码失败".to_string())?;
    if checksum(&json) != sum.to_ascii_lowercase() {
        return Err("迁移码校验失败，可能复制时少了一段".into());
    }
    import_json(&json, now_ms)
}

fn sanitize(data: &J, now_ms: i64) -> Result<Save, String> {
    let mut save = Save::default();
    let num = |key: &str, min: i32, max: i32, fallback: i32| -> i32 {
        let raw = match data.get(key) {
            Some(J::Str(s)) => s.as_str(),
            Some(J::Num(n)) => return (*n as i32).clamp(min, max),
            _ => return fallback,
        };
        raw.parse::<f64>().ok().map(|n| (n.round() as i32).clamp(min, max)).unwrap_or(fallback)
    };
    save.growth.mood = num("mood", 0, MOOD_MAX, 70);
    save.growth.affinity = num("affinity", 0, AFFINITY_MAX, 0);
    save.growth.satiety = num("satiety", 0, SATIETY_MAX, 80) as f64;
    save.growth.signin_streak = num("signinStreak", 0, 10_000, 0);
    save.growth.level = num("level", 1, 999, 1);
    if let Some(s) = data.get("lastSignin").and_then(|v| v.as_str()) {
        save.growth.last_signin = if is_day(s) { s.to_string() } else { String::new() };
    }
    if let Some(s) = data.get("achievements").and_then(|v| v.as_str()) {
        save.growth.achievements = clean_achievements(s);
    }
    if let Some(v) = data.get("companionSince") {
        let n = match v {
            J::Str(s) => s.parse::<i64>().ok(),
            J::Num(n) => Some(*n as i64),
            _ => None,
        };
        if let Some(n) = n {
            if n > 0 && n <= now_ms + 86_400_000 {
                save.companion_since = n;
            }
        }
    }
    if let Some(s) = data.get("badge").and_then(|v| v.as_str()) {
        save.badge = if s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            s.to_string()
        } else { String::new() };
    }
    save.title = data.get("title").and_then(|v| v.as_str()).map(|s| clip(&clean_text(s), 20)).unwrap_or_default();
    save.self_name = data.get("selfName").and_then(|v| v.as_str()).map(|s| clip(&clean_text(s), 20)).unwrap_or_default();
    save.quests = keep_json(data, "quests", |v| {
        v.get("slots").and_then(|s| s.as_arr()).map(|a| a.len() <= 3).unwrap_or(false)
    });
    save.week = keep_json(data, "weekSignin", |v| matches!(v, J::Obj(_)));
    save.game = keep_json(data, "gameStats", |v| matches!(v, J::Obj(_)));
    save.journal = keep_json(data, "journal", |v| v.as_arr().map(|a| a.len() <= 80).unwrap_or(false));
    if let Some(raw) = data.get("usageStats").and_then(|v| v.as_str()) {
        if raw.len() <= MAX_JSON {
            if let Some(obj) = crate::json::parse(raw) {
                let tools_ok = match obj.get("tools") {
                    None => true,
                    Some(J::Num(_)) => true,
                    _ => false,
                };
                if tools_ok && matches!(obj, J::Obj(_)) {
                    let n = |k: &str| obj.get(k).and_then(|v| v.as_num()).unwrap_or(0.0).max(0.0) as u32;
                    save.usage.tools = n("tools");
                    save.usage.code = n("code");
                    save.usage.success = n("success");
                    save.usage.failure = n("failure");
                    save.usage.messages = n("messages");
                    save.usage.pats = n("pats");
                    save.usage.feeds = n("feeds");
                    save.usage.triples = n("triples");
                }
            }
        }
    }
    save.relevel();
    if save.growth.mood == 0 && save.growth.affinity == 0 && save.growth.achievements.is_empty()
        && save.title.is_empty() && save.companion_since == 0 && save.journal.is_empty()
        && data.get("mood").is_none() && data.get("affinity").is_none()
    {
        return Err("存档里没有可识别的养成数据".into());
    }
    Ok(save)
}

fn keep_json(data: &J, key: &str, ok: impl Fn(&J) -> bool) -> String {
    let Some(raw) = data.get(key).and_then(|v| v.as_str()) else { return String::new() };
    if raw.is_empty() || raw.len() > MAX_JSON { return String::new(); }
    match crate::json::parse(raw) {
        Some(v) if ok(&v) => v.dump(),
        _ => String::new(),
    }
}

fn clean_achievements(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    for id in value.split(',') {
        let id = id.trim();
        if id.is_empty() || out.iter().any(|s: &String| s == id) { continue; }
        if ACHIEVEMENTS.iter().any(|(k, _)| *k == id) {
            out.push(id.to_string());
        }
        if out.len() >= 120 { break; }
    }
    out
}

fn clean_text(value: &str) -> String {
    value.chars().filter(|c| (*c as u32) >= 0x20 && *c != '\u{7f}').collect::<String>().trim().to_string()
}

fn clip(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn is_day(s: &str) -> bool {
    let mut p = s.split('-');
    let (Some(y), Some(m), Some(d), None) = (p.next(), p.next(), p.next(), p.next()) else { return false };
    y.len() == 4 && y.chars().all(|c| c.is_ascii_digit())
        && (1..=2).contains(&m.len()) && m.chars().all(|c| c.is_ascii_digit())
        && (1..=2).contains(&d.len()) && d.chars().all(|c| c.is_ascii_digit())
}

pub fn checksum(text: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for unit in text.encode_utf16() {
        hash ^= unit as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

fn base64_encode(text: &str) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] as u32 } else { 0 };
        out.push(T[(b0 >> 2) as usize] as char);
        out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if i + 1 < bytes.len() { T[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char } else { '=' });
        out.push(if i + 2 < bytes.len() { T[(b2 & 63) as usize] as char } else { '=' });
        i += 3;
    }
    out
}

fn base64_decode(text: &str) -> Result<String, ()> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut padded = text.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/').collect::<String>();
    while padded.len() % 4 != 0 { padded.push('='); }
    let bytes_in: Vec<u8> = padded.bytes().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes_in.len() {
        if bytes_in[i] == b'=' { break; }
        let c0 = T.iter().position(|c| *c == bytes_in[i]).ok_or(())? as u32;
        let c1 = T.iter().position(|c| *c == bytes_in[i + 1]).ok_or(())? as u32;
        out.push(((c0 << 2) | (c1 >> 4)) as u8);
        if i + 2 < bytes_in.len() && bytes_in[i + 2] != b'=' {
            let c2 = T.iter().position(|c| *c == bytes_in[i + 2]).ok_or(())? as u32;
            out.push((((c1 & 15) << 4) | (c2 >> 2)) as u8);
            if i + 3 < bytes_in.len() && bytes_in[i + 3] != b'=' {
                let c3 = T.iter().position(|c| *c == bytes_in[i + 3]).ok_or(())? as u32;
                out.push((((c2 & 3) << 6) | c3) as u8);
            }
        }
        i += 4;
    }
    String::from_utf8(out).map_err(|_| ())
}

/// On-disk file. The `dst` object is not part of an export.
pub fn save_path() -> std::path::PathBuf {
    crate::config::home().join("pet.json")
}

pub fn load_file(path: &std::path::Path) -> Save {
    let Ok(text) = std::fs::read_to_string(path) else { return Save::default() };
    let Some(root) = crate::json::parse(&text) else { return Save::default() };
    let Some(data) = root.get("data") else { return Save::default() };
    let mut save = sanitize(data, now_ms()).unwrap_or_default();
    if let Some(n) = root.get("lastSeen").and_then(|v| v.as_num()) {
        save.last_seen = n as i64;
    }
    save
}

pub fn store_file(path: &std::path::Path, save: &Save) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let body = J::o(vec![
        ("schema", J::s("dst-pet/save")),
        ("version", J::Num(1.0)),
        ("lastSeen", J::Num(save.last_seen as f64)),
        ("data", data_object(save, false)),
    ]).dump();
    // Refuse to persist anything that looks like a credential. The save struct
    // has no key field; this is a backstop against a future field leaking one.
    let low = body.to_ascii_lowercase();
    if low.contains("api_key") || low.contains("dsh_token") || low.contains("authorization") || low.contains("sk-") {
        return Err("pet save refused: credential-like text".into());
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pat_feed_and_level_match_the_reference_rules() {
        let mut s = Save::default();
        let now = 1_763_600_000_000;
        s.companion_since = now;
        let u = s.apply(GEvent::Pat, now);
        assert_eq!(s.growth.mood, 74);
        assert_eq!(s.growth.affinity, 2);
        assert!(u.contains(&"first-pat".to_string()));
        s.apply(GEvent::Feed, now);
        assert_eq!(s.growth.satiety, 100.0); // 80 + 30 clamped
        assert_eq!(s.growth.affinity, 7);
        assert!(s.growth.achievements.iter().any(|a| a == "first-feed"));
        s.growth.affinity = 500;
        s.relevel();
        assert_eq!(s.growth.level, 2);
        s.growth.affinity = 2500;
        s.relevel();
        assert_eq!(s.growth.level, 6);
        assert!(s.apply(GEvent::Tick { mins: 0.0 }, now).iter().any(|a| a == "lv5"));
    }

    #[test]
    fn signin_streak_uses_local_days() {
        let mut s = Save::default();
        let now = now_ms();
        s.apply(GEvent::Signin, now);
        assert_eq!(s.growth.signin_streak, 1);
        assert_eq!(s.growth.last_signin, local_day(now));
        let again = s.apply(GEvent::Signin, now);
        assert!(again.iter().all(|a| a != "signin3"));
        assert_eq!(s.growth.signin_streak, 1, "same day does not increment");
    }

    #[test]
    fn wm1_roundtrip_and_rejection() {
        let mut s = Save::default();
        s.growth.mood = 64;
        s.growth.affinity = 42;
        s.title = "主人".into();
        s.self_name = "大肥鱼".into();
        s.growth.achievements = vec!["first-pat".into(), "nope".into()];
        s.quests = r#"{"slots":[{"id":"signin-1"}]}"#.into();
        let now = 1_763_600_000_000;
        let code = export_code(&s, now, "dst 0.2.0");
        assert!(code.starts_with("WM1-"));
        assert!(code.matches('-').count() >= 2, "base64url may contain hyphens");
        // Locked against whale-moe-core.js portableCode at the reference commit
        // (same JSON, same FNV-1a, unknown achievement ids dropped before encode).
        assert_eq!(
            code,
            "WM1-0aee90d8-eyJzY2hlbWEiOiJkc2gtd2hhbGUtbXVzdW1lL3NhdmUiLCJ2ZXJzaW9uIjoxLCJleHBvcnRlZEF0IjoiMjAyNS0xMS0yMFQwMDo1MzoyMC4wMDBaIiwiYXBwVmVyc2lvbiI6ImRzdCAwLjIuMCIsImRhdGEiOnsibW9vZCI6IjY0IiwiYWZmaW5pdHkiOiI0MiIsInNhdGlldHkiOiI4MCIsImxhc3RTaWduaW4iOiIiLCJzaWduaW5TdHJlYWsiOiIwIiwiYWNoaWV2ZW1lbnRzIjoiZmlyc3QtcGF0IiwiY29tcGFuaW9uU2luY2UiOiIiLCJsZXZlbCI6IjEiLCJiYWRnZSI6IiIsInRpdGxlIjoi5Li75Lq6Iiwic2VsZk5hbWUiOiLlpKfogqXpsbwifX0"
        );
        let back = import_text(&code, now).unwrap();
        assert_eq!(back.growth.mood, 64);
        assert_eq!(back.growth.affinity, 42);
        assert_eq!(back.title, "主人");
        assert_eq!(back.self_name, "大肥鱼");
        assert_eq!(back.growth.achievements, vec!["first-pat".to_string()]);
        // Quests ride the JSON file, not the short WM1 code (the reference core omits them).
        let file = export_json(&s, now, "dst 0.2.0");
        let from_file = import_text(&file, now).unwrap();
        assert!(from_file.quests.contains("signin-1"));
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        let _ = std::fs::create_dir_all(&root);
        let _ = std::fs::write(root.join("pet-wm1-fixture.txt"), &code);
        let _ = std::fs::write(root.join("pet-json-fixture.json"), &file);
        assert!(import_text("WM1-deadbeef-aaaa", now).is_err());
        assert!(import_text("{\"schema\":\"other\",\"version\":1,\"data\":{}}", now).is_err());
        let bad = import_text(&code.replace('A', "B").replace('a', "b"), now);
        assert!(bad.is_err(), "a damaged code must not apply");
        // Produced by the reference portableCode. The payload contains '-' so a
        // split on every hyphen would reject it.
        let from_js = "WM1-8d62d3e2-eyJzY2hlbWEiOiJkc2gtd2hhbGUtbXVzdW1lL3NhdmUiLCJ2ZXJzaW9uIjoxLCJleHBvcnRlZEF0IjoiMjAyNS0xMS0yMFQwMDo1MzoyMC4wMDBaIiwiYXBwVmVyc2lvbiI6InJlZiIsImRhdGEiOnsibW9vZCI6IjEyIiwiYWZmaW5pdHkiOiIzIiwic2F0aWV0eSI6IjQwIiwibGFzdFNpZ25pbiI6IjIwMjYtMS0yIiwic2lnbmluU3RyZWFrIjoiMiIsImFjaGlldmVtZW50cyI6ImZpcnN0LXBhdCIsImNvbXBhbmlvblNpbmNlIjoiMTc2MzYwMDAwMDAwMCIsImxldmVsIjoiMSIsImpvdXJuYWwiOiJbe1wiYXRcIjoxLFwia2luZFwiOlwicGF0XCIsXCJ0ZXh0XCI6XCJoaVwifV0iLCJiYWRnZSI6IiIsInRpdGxlIjoiQSIsInNlbGZOYW1lIjoiQiJ9fQ";
        let js = import_text(from_js, now).unwrap();
        assert_eq!(js.growth.mood, 12);
        assert_eq!(js.growth.last_signin, "2026-1-2");
        assert_eq!(js.title, "A");
        assert!(js.journal.contains("pat"));
        // base64url writes '+' as '-'. This JSON's payload contains that byte,
        // so a splitter that cuts on every hyphen would drop the tail.
        let hyphen_json = r#"{"schema":"dsh-whale-musume/save","version":1,"exportedAt":"2025-11-20T00:53:20.000Z","appVersion":"dst","data":{"mood":"1","title":"A>"}}"#;
        let b64 = base64_encode(hyphen_json).replace('+', "-").replace('/', "_").trim_end_matches('=').to_string();
        assert!(b64.contains('-'), "{b64}");
        let hyphen_code = format!("WM1-{}-{b64}", checksum(hyphen_json));
        let hyphenated = import_text(&hyphen_code, now).unwrap();
        assert_eq!(hyphenated.title, "A>");
        assert_eq!(hyphenated.growth.mood, 1);
    }

    #[test]
    fn export_has_no_secret_fields() {
        let json = export_json(&Save::default(), 1_763_600_000_000, "dst");
        let low = json.to_ascii_lowercase();
        for needle in ["api_key", "dsh_token", "authorization", "sk-", "bearer"] {
            assert!(!low.contains(needle), "{needle} in {json}");
        }
    }

    #[test]
    fn catalog_has_the_reference_forty_five() {
        assert_eq!(ACHIEVEMENTS.len(), 45);
        let triggers: Vec<_> = ACHIEVEMENTS.iter().filter(|(id, _)| device_can_trigger(id)).collect();
        assert_eq!(triggers.len(), 29);
    }
}
