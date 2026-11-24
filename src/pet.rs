//! Whale-girl session: DSH events in, a pose and a picture out.
//!
//! The machine is pure aside from the clock the caller passes in. It does not
//! read the config file, the transcript or tool arguments.

use crate::pet_art::{self, Pack, Paint, Scene};
use crate::pet_growth::{self, GEvent, Save};

pub const DOCK_COLS: usize = 10;
const HOLD_MS: u64 = 600;
const SLEEP_MS: u64 = 10_000;
const FAIL_MS: u64 = 1_500;
const SUCCESS_MS: u64 = 2_100;
const INTERACT_MS: u64 = 1_200;
const TRIPLE_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shell, Read, Web, Write, Look, Plan, Schedule, Voice, Ask, Screen, Generic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Idle, Sleep, Think, Talk, Tool, Success, Failure, Compact, Listen, Startled, Eat, Blush, Happy,
}

impl Pose {
    pub fn sprite(self) -> &'static str {
        match self {
            Pose::Idle => "idle",
            Pose::Sleep => "sleep",
            Pose::Think | Pose::Listen => "think",
            Pose::Talk => "talk",
            Pose::Tool => "tool",
            Pose::Success | Pose::Happy => "happy",
            Pose::Failure | Pose::Startled => "sad",
            Pose::Compact | Pose::Eat => "eat",
            Pose::Blush => "blush",
        }
    }
    fn rank(self) -> u8 {
        match self {
            Pose::Failure => 90,
            Pose::Compact => 80,
            Pose::Eat | Pose::Blush | Pose::Happy => 70,
            Pose::Tool => 60,
            Pose::Think | Pose::Listen => 50,
            Pose::Talk => 40,
            Pose::Success => 35,
            Pose::Startled => 25,
            Pose::Sleep => 10,
            Pose::Idle => 0,
        }
    }
    pub fn prop(self, kind: Kind) -> &'static str {
        match self {
            Pose::Compact => "plan",
            Pose::Tool => match kind {
                Kind::Shell => "shell",
                Kind::Read => "read",
                Kind::Web => "web",
                Kind::Write => "write",
                Kind::Look => "look",
                Kind::Plan => "plan",
                Kind::Schedule => "clock",
                Kind::Voice => "note",
                Kind::Ask => "ask",
                Kind::Screen => "artifact",
                Kind::Generic => "gear",
            },
            _ => "",
        }
    }
}

pub fn kind_of(name: &str) -> Option<Kind> {
    if name == "state_report" {
        return None;
    }
    Some(match name {
        "bash" | "job_start" | "job_wait" | "job_output" | "job_kill" | "job_list" | "app_run" | "app_test" => Kind::Shell,
        "read_file" | "list_dir" | "glob" | "grep" | "skill" | "todo_read" | "get_goal" | "memory_read" | "memory_search" | "watcher_list" => Kind::Read,
        "search" | "fetch" => Kind::Web,
        "write_file" | "edit_file" | "skill_save" | "skill_file" | "skill_rollback" | "memory_write" | "rename" => Kind::Write,
        "read_image" | "ocr" => Kind::Look,
        "todo_write" | "create_goal" | "update_goal" => Kind::Plan,
        "cron" | "watch" | "watcher_start" | "watcher_stop" => Kind::Schedule,
        "speak" => Kind::Voice,
        "ask_user" => Kind::Ask,
        "artifact" => Kind::Screen,
        _ => Kind::Generic,
    })
}

pub fn kind_is_code(kind: Kind) -> bool {
    matches!(kind, Kind::Shell)
}

#[derive(Clone, Debug)]
pub struct Machine {
    tools_open: u32,
    kind: Kind,
    thinking: bool,
    talking: bool,
    compacting: bool,
    listening: bool,
    failure_until: u64,
    success_until: u64,
    interact_until: u64,
    interact: Pose,
    startled_until: u64,
    last_activity: u64,
    shown: Pose,
    shown_at: u64,
    turn_error: bool,
    recent_pats: Vec<u64>,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            tools_open: 0,
            kind: Kind::Generic,
            thinking: false,
            talking: false,
            compacting: false,
            listening: false,
            failure_until: 0,
            success_until: 0,
            interact_until: 0,
            interact: Pose::Blush,
            startled_until: 0,
            last_activity: 0,
            shown: Pose::Idle,
            shown_at: 0,
            turn_error: false,
            recent_pats: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Done,
    Failed,
    Cancelled,
    /// The configured step cap stopped the turn. Not a success and not a failure.
    Limited,
}

#[derive(Clone, Debug)]
pub enum Ev {
    TurnStart,
    Thinking,
    ThinkingDone,
    Talk,
    TalkDone,
    ToolCall(String),
    ToolResult { name: String, is_error: bool },
    Compact,
    CompactDone { ok: bool },
    /// `Failed` is an API or harness failure that has not already been counted
    /// as a tool error. `Cancelled` is Esc. Neither one is a success.
    TurnEnd(End),
    Esc,
    Listen(bool),
    Pat,
    Feed,
    Praise,
}

impl Machine {
    pub fn on(&mut self, ev: Ev, now: u64) -> Option<GEvent> {
        self.last_activity = now;
        let growth = match ev {
            Ev::TurnStart => {
                self.turn_error = false;
                self.talking = false;
                None
            }
            Ev::Thinking => { self.thinking = true; self.talking = false; None }
            Ev::ThinkingDone => { self.thinking = false; None }
            Ev::Talk => { self.talking = true; self.thinking = false; None }
            Ev::TalkDone => { self.talking = false; None }
            Ev::ToolCall(name) => {
                self.thinking = false;
                if let Some(kind) = kind_of(&name) {
                    self.tools_open = self.tools_open.saturating_add(1);
                    self.kind = kind;
                }
                None
            }
            Ev::ToolResult { name, is_error } => {
                if kind_of(&name).is_none() {
                    None
                } else {
                self.tools_open = self.tools_open.saturating_sub(1);
                if is_error {
                    self.failure_until = now + FAIL_MS;
                    self.turn_error = true;
                    Some(GEvent::Failure)
                } else {
                    None
                }
                }
            }
            Ev::Compact => { self.compacting = true; None }
            Ev::CompactDone { ok } => {
                self.compacting = false;
                if ok { None } else {
                    self.failure_until = now + FAIL_MS;
                    self.turn_error = true;
                    Some(GEvent::Failure)
                }
            }
            Ev::TurnEnd(end) => {
                self.thinking = false;
                self.talking = false;
                self.tools_open = 0;
                self.compacting = false;
                let already = self.turn_error;
                self.turn_error = false;
                match end {
                    End::Done if !already => {
                        self.success_until = now + SUCCESS_MS;
                        Some(GEvent::Success)
                    }
                    End::Failed if !already => {
                        self.failure_until = now + FAIL_MS;
                        Some(GEvent::Failure)
                    }
                    End::Limited | End::Cancelled | End::Done | End::Failed => None,
                }
            }
            Ev::Esc => {
                let in_turn = self.thinking || self.talking || self.tools_open > 0 || self.compacting;
                self.thinking = false;
                self.talking = false;
                self.tools_open = 0;
                self.compacting = false;
                self.startled_until = now + 900;
                if in_turn {
                    self.turn_error = true;
                }
                None
            }
            Ev::Listen(on) => { self.listening = on; None }
            Ev::Pat => {
                self.recent_pats.push(now);
                self.recent_pats.retain(|t| now - *t <= TRIPLE_MS);
                self.interact = Pose::Blush;
                self.interact_until = now + INTERACT_MS;
                if self.recent_pats.len() >= 3 {
                    self.recent_pats.clear();
                    self.interact = Pose::Happy;
                    Some(GEvent::Triple)
                } else {
                    Some(GEvent::Pat)
                }
            }
            Ev::Feed => {
                self.interact = Pose::Eat;
                self.interact_until = now + INTERACT_MS;
                Some(GEvent::Feed)
            }
            Ev::Praise => {
                self.interact = Pose::Happy;
                self.interact_until = now + INTERACT_MS;
                Some(GEvent::Praise)
            }
        };
        let _ = self.pose(now);
        growth
    }

    pub fn pose(&mut self, now: u64) -> Pose {
        let want = self.desired(now);
        if self.shown_at == 0
            || now.saturating_sub(self.shown_at) >= HOLD_MS
            || want.rank() > self.shown.rank()
        {
            if want != self.shown {
                self.shown = want;
                self.shown_at = now;
            }
        }
        self.shown
    }

    pub fn kind(&self) -> Kind { self.kind }

    fn desired(&self, now: u64) -> Pose {
        if now < self.failure_until { return Pose::Failure; }
        if self.compacting { return Pose::Compact; }
        if now < self.interact_until { return self.interact; }
        if self.tools_open > 0 { return Pose::Tool; }
        if self.thinking { return Pose::Think; }
        if self.talking { return Pose::Talk; }
        if now < self.success_until { return Pose::Success; }
        if self.listening { return Pose::Listen; }
        if now < self.startled_until { return Pose::Startled; }
        if now.saturating_sub(self.last_activity) >= SLEEP_MS { return Pose::Sleep; }
        Pose::Idle
    }
}

/// What the screen is doing with the pet right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Text,
    Pixel,
}

#[derive(Clone, Debug)]
pub struct Live {
    pub enabled: bool,
    pub dock: bool,
    pub room: bool,
    /// `auto`, `pixel` or `text`.
    pub render: String,
    pub mode: Mode,
    pub machine: Machine,
    pub save: Save,
    pub pack: Option<Pack>,
    pub pack_error: String,
    room_live: bool,
    last_decay: u64,
    pub dirty: bool,
    /// Counters changed and should be written. Pose-only events leave this false.
    pub needs_save: bool,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            enabled: false,
            dock: true,
            room: true,
            render: "auto".into(),
            mode: Mode::Off,
            machine: Machine::default(),
            save: Save::default(),
            pack: None,
            pack_error: String::new(),
            room_live: false,
            last_decay: 0,
            dirty: true,
            needs_save: false,
        }
    }
}

impl Live {
    pub fn configure(&mut self, enabled: bool, dock: bool, room: bool, render: &str) {
        self.enabled = enabled;
        self.dock = dock;
        self.room = room;
        self.render = match render {
            "pixel" | "text" | "auto" => render.to_string(),
            _ => "auto".into(),
        };
        if !self.enabled {
            self.mode = Mode::Off;
            self.room_live = false;
        }
        self.dirty = true;
    }

    /// Decide pixel vs text. `pixel_possible` is true when a probed panel or
    /// the offscreen sink can take the picture. Pixel mode also needs the
    /// original pack. Without it the character whale girl is used, never the
    /// simplified builtin sprite. Never writes a device itself.
    pub fn resolve(&mut self, pixel_possible: bool) {
        if !self.enabled {
            self.mode = Mode::Off;
            return;
        }
        let pixel = pixel_possible && self.pack.is_some();
        self.mode = match self.render.as_str() {
            "text" => Mode::Text,
            _ if pixel => Mode::Pixel,
            _ => Mode::Text,
        };
        self.dirty = true;
    }

    pub fn has_pack(&self) -> bool {
        self.pack.is_some()
    }

    #[cfg(test)]
    pub fn set_pack_for_test(&mut self, pack: pet_art::Pack) {
        self.pack = Some(pack);
    }

    pub fn enter_room(&mut self) {
        self.room_live = self.enabled && self.room;
        self.dirty = true;
    }

    pub fn enter_chat(&mut self) {
        self.room_live = false;
        self.dirty = true;
    }

    pub fn dock_showing(&self) -> bool {
        self.mode == Mode::Pixel && self.dock && self.enabled && !self.room_live
    }

    pub fn room_showing(&self) -> bool {
        self.mode == Mode::Pixel && self.room_live
    }

    pub fn text_room(&self) -> bool {
        self.mode == Mode::Text && self.room_live && self.room
    }

    pub fn wrap_cols(&self, full: usize) -> usize {
        if self.dock_showing() { full.saturating_sub(DOCK_COLS).max(24) } else { full }
    }

    pub fn on_event(&mut self, ev: Ev, now: u64) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let was = self.machine.pose(now);
        let growth = self.machine.on(ev.clone(), now);
        let mut persist = growth.is_some();
        let pose = self.machine.pose(now);
        if pose != was {
            self.dirty = true;
        }
        let mut unlocked = Vec::new();
        if let Some(g) = growth {
            unlocked.extend(self.save.apply(g, now as i64));
        }
        if let Ev::ToolCall(name) = &ev {
            if let Some(kind) = kind_of(name) {
                persist = true;
                unlocked.extend(self.save.note_tool(kind_is_code(kind), now as i64));
                if pet_growth::night_hour(pet_growth::local_hour(now as i64)) && self.save.grant_id("night-work") {
                    unlocked.push("night-work".into());
                }
            }
        }
        if matches!(ev, Ev::TalkDone) {
            persist = true;
            unlocked.extend(self.save.note_message(now as i64));
        }
        if matches!(ev, Ev::Pat | Ev::Feed | Ev::Praise)
            && pet_growth::night_hour(pet_growth::local_hour(now as i64))
            && self.save.grant_id("night-owl")
        {
            unlocked.push("night-owl".into());
        }
        for id in &unlocked {
            self.save.remember(now as i64, "achievement", &format!("{} {}", id, pet_growth::achievement_name(id)));
        }
        // Coming back after two hours.
        if self.save.last_seen > 0 && now as i64 - self.save.last_seen > 2 * 3_600_000 && self.save.grant_id("comeback") {
            unlocked.push("comeback".into());
        }
        self.save.last_seen = now as i64;
        if self.last_decay == 0 { self.last_decay = now; }
        let mins = now.saturating_sub(self.last_decay) as f64 / 60_000.0;
        if mins >= 1.0 {
            persist = true;
            unlocked.extend(self.save.apply(GEvent::Tick { mins }, now as i64));
            self.last_decay = now;
        }
        if persist {
            self.needs_save = true;
        }
        unlocked
    }

    pub fn sign_in(&mut self, now: u64) -> Vec<String> {
        if !self.enabled { return Vec::new(); }
        if self.save.companion_since == 0 {
            self.save.companion_since = now as i64;
        }
        self.save.apply(GEvent::Signin, now as i64)
    }

    pub fn paint(&mut self, now: u64) -> Option<Vec<u8>> {
        if self.mode != Mode::Pixel { return None; }
        let scene = if self.room_showing() { Scene::Room } else if self.dock_showing() { Scene::Dock } else { return None };
        let pose = self.machine.pose(now);
        let tick = now / 300;
        let sprite = pet_art::pose_sprite(pose.sprite(), tick);
        let mut paint = Paint::new(scene, sprite, pose.prop(self.machine.kind()));
        paint.mood = self.save.mood_pct();
        paint.satiety = self.save.satiety_pct();
        paint.bond = self.save.bond_pct();
        paint.level = self.save.growth.level.max(1) as u16;
        self.dirty = false;
        Some(pet_art::render(&paint, self.pack.as_ref()))
    }

    pub fn text_rows(&mut self, now: u64) -> Option<&'static [&'static str]> {
        if !self.text_room() { return None; }
        let pose = self.machine.pose(now);
        Some(pet_art::text_rows(pet_art::pose_sprite(pose.sprite(), now / 300)))
    }

    pub fn badge(&mut self, now: u64) -> Option<&'static str> {
        if self.mode != Mode::Text || self.room_live || !self.enabled { return None; }
        Some(pet_art::badge(self.machine.pose(now).sprite()))
    }

    pub fn load_pack(&mut self, dir: &std::path::Path) {
        match pet_art::load_pack(dir) {
            Ok(Some(pack)) => { self.pack = Some(pack); self.pack_error.clear(); }
            Ok(None) => { self.pack = None; self.pack_error.clear(); }
            Err(e) => { self.pack = None; self.pack_error = e; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_drive_poses_and_hold() {
        let mut m = Machine::default();
        m.on(Ev::Thinking, 0);
        assert_eq!(m.pose(0), Pose::Think);
        m.on(Ev::ToolCall("bash".into()), 100);
        assert_eq!(m.pose(100), Pose::Tool);
        assert_eq!(m.kind(), Kind::Shell);
        m.on(Ev::ToolResult { name: "bash".into(), is_error: false }, 200);
        // The tool pose holds for 600ms so a fast call does not flash away.
        assert_eq!(m.pose(200), Pose::Tool);
        assert_eq!(m.pose(800), Pose::Idle);
        m.on(Ev::ToolCall("state_report".into()), 1000);
        assert_eq!(m.pose(1000), Pose::Idle, "state_report is not a work pose");
    }

    #[test]
    fn failure_cuts_through_and_success_follows_a_clean_turn() {
        let mut m = Machine::default();
        m.on(Ev::ToolCall("bash".into()), 0);
        m.on(Ev::ToolResult { name: "bash".into(), is_error: true }, 100);
        assert_eq!(m.pose(100), Pose::Failure);
        m.on(Ev::TurnEnd(End::Done), 200);
        assert_eq!(m.pose(2000), Pose::Idle, "an errored turn does not celebrate");
        m.on(Ev::TurnEnd(End::Done), 3000);
        assert_eq!(m.pose(3000), Pose::Success);
    }

    #[test]
    fn api_failure_and_cancel_are_not_successes() {
        let mut live = Live::default();
        live.configure(true, true, true, "text");
        live.resolve(false);
        live.on_event(Ev::TurnEnd(End::Failed), 1_000);
        assert_eq!(live.save.usage.failure, 1);
        assert_eq!(live.save.usage.success, 0);
        live.on_event(Ev::TurnStart, 2_000);
        assert_eq!(live.on_event(Ev::TurnEnd(End::Cancelled), 2_100).len(), 0);
        assert_eq!(live.save.usage.success, 0);
        assert_eq!(live.save.usage.failure, 1);
        live.on_event(Ev::ToolCall("bash".into()), 3_000);
        live.on_event(Ev::ToolResult { name: "bash".into(), is_error: true }, 3_100);
        live.on_event(Ev::TurnEnd(End::Failed), 3_200);
        assert_eq!(live.save.usage.failure, 2, "the tool error is counted once");
        assert_eq!(live.save.usage.success, 0);
        live.on_event(Ev::TurnStart, 4_000);
        live.on_event(Ev::TurnEnd(End::Done), 4_100);
        assert_eq!(live.save.usage.success, 1);
        assert!(live.needs_save);
        live.on_event(Ev::TurnStart, 5_000);
        live.on_event(Ev::TurnEnd(End::Limited), 5_100);
        assert_eq!(live.save.usage.success, 1, "a step cap is not a success");
        assert_eq!(live.save.usage.failure, 2);
    }

    #[test]
    fn idle_ten_seconds_sleeps_and_a_pat_wakes() {
        let mut m = Machine::default();
        m.on(Ev::TurnEnd(End::Done), 0);
        assert_eq!(m.pose(9_000), Pose::Idle);
        assert_eq!(m.pose(11_000), Pose::Sleep);
        assert_eq!(m.on(Ev::Pat, 12_000), Some(GEvent::Pat));
        assert_eq!(m.pose(12_000), Pose::Blush);
    }

    #[test]
    fn every_builtin_tool_has_a_kind() {
        for name in [
            "bash", "read_file", "write_file", "edit_file", "list_dir", "glob", "grep",
            "search", "fetch", "ocr", "read_image", "artifact", "speak", "rename",
            "todo_read", "todo_write", "skill", "state_report", "job_start", "job_output",
            "job_wait", "job_kill", "job_list", "watcher_start", "watcher_list", "watcher_stop",
            "get_goal", "create_goal", "update_goal", "memory_write", "memory_read", "memory_search",
            "cron", "watch", "app_run", "app_test", "skill_save", "skill_file", "skill_rollback", "ask_user",
        ] {
            if name == "state_report" {
                assert!(kind_of(name).is_none());
            } else {
                assert!(kind_of(name).is_some(), "{name}");
            }
        }
        assert_eq!(kind_of("mcp__srv__x"), Some(Kind::Generic));
    }

    #[test]
    fn dock_narrows_only_while_it_is_showing() {
        let mut live = Live::default();
        assert_eq!(live.wrap_cols(53), 53);
        live.configure(true, true, true, "pixel");
        live.resolve(true);
        assert!(!live.dock_showing(), "no pack means the character portrait, not a pixel dock");
        assert_eq!(live.wrap_cols(53), 53);
        live.set_pack_for_test(pet_art::Pack::from_idle_pixel());
        live.resolve(true);
        live.enter_chat();
        assert!(live.dock_showing());
        assert_eq!(live.wrap_cols(53), 43);
        live.configure(true, false, true, "pixel");
        live.resolve(true);
        assert_eq!(live.wrap_cols(53), 53);
    }
}
