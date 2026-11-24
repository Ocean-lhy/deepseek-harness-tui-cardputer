//! Screen-side whale girl. The picture comes from `pet_art`; the mood comes
//! from `pet_growth`. This module only decides when to show which, and it
//! never reads the API key, the transcript or a tool argument.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::cell::RefCell;

use crate::config::Config;
use crate::pet::{self, Ev as PetEv};
use crate::pet_growth;

pub type PetHandle = Rc<RefCell<Host>>;

pub struct Host {
    pub live: pet::Live,
    panel: Option<crate::fb::Panel>,
    tried: bool,
    fb_override: String,
    out_path: Option<PathBuf>,
    pack_dir: PathBuf,
    save_path: PathBuf,
    pub last_save_error: String,
}

pub enum IdleFace {
    Hidden,
    Pixel,
    Text(&'static [&'static str]),
}

impl Host {
    pub fn boot(cfg: &Config) -> Self {
        let mut host = Self {
            live: pet::Live::default(),
            panel: None,
            tried: false,
            fb_override: std::env::var("DST_FB").unwrap_or_else(|_| cfg.pet_fb.clone()),
            out_path: std::env::var("DST_PET_FB_OUT").ok().map(PathBuf::from),
            pack_dir: pack_dir_from_env(),
            save_path: pet_growth::save_path(),
            last_save_error: String::new(),
        };
        host.apply_cfg(cfg);
        host.live.save = pet_growth::load_file(&host.save_path);
        host.live.load_pack(&host.pack_dir);
        host.resolve();
        // last_activity starts at 0; without this the girl is born asleep
        // because "now" is milliseconds since the epoch.
        let _ = host.live.machine.on(pet::Ev::Listen(false), now());
        if host.live.enabled {
            let now = now();
            let _ = host.live.sign_in(now);
            host.persist_save();
        }
        host
    }

    fn persist_save(&mut self) {
        match pet_growth::store_file(&self.save_path, &self.live.save) {
            Ok(()) => self.last_save_error.clear(),
            Err(e) => self.last_save_error = e,
        }
    }

    pub fn apply_cfg(&mut self, cfg: &Config) {
        self.live.configure(cfg.pet_enabled, cfg.pet_dock, cfg.pet_room, &cfg.pet_render);
        if std::env::var("DST_FB").is_err() {
            self.fb_override = cfg.pet_fb.clone();
        }
    }

    /// Pixel when a probed panel or the offscreen PPM sink can take the
    /// picture. An unknown framebuffer is not opened.
    pub fn resolve(&mut self) {
        let mut pixel = self.out_path.is_some();
        if self.live.enabled && self.live.render != "text" {
            let explicit = !self.fb_override.trim().is_empty();
            let allowed = explicit
                || std::env::var_os("DST_PET_FB_FORCE").is_some()
                || crate::fb::fbterm_ancestor();
            if self.panel.is_some() {
                pixel = true;
            } else if allowed && !self.tried {
                self.tried = true;
                match crate::fb::open(self.fb_override.trim()) {
                    Ok(panel) => {
                        self.panel = Some(panel);
                        pixel = true;
                    }
                    Err(_) => {}
                }
            }
        }
        self.live.resolve(pixel);
    }

    pub fn follow(&mut self, idle_splash: bool) {
        if self.live.enabled && idle_splash && self.live.room {
            self.live.enter_room();
        } else if self.live.room_showing() || self.live.text_room() {
            self.live.enter_chat();
        } else if !idle_splash {
            self.live.enter_chat();
        }
    }

    pub fn dock_width(&mut self, full: usize) -> usize {
        if self.live.dock_showing() { self.live.wrap_cols(full) } else { 0 }
    }

    pub fn badge(&mut self) -> Option<&'static str> {
        self.live.badge(now())
    }

    pub fn idle_face(&mut self) -> IdleFace {
        if !self.live.enabled || !self.live.room {
            return IdleFace::Hidden;
        }
        self.live.enter_room();
        if self.live.room_showing() {
            return IdleFace::Pixel;
        }
        if self.live.text_room() {
            if let Some(rows) = self.live.text_rows(now()) {
                return IdleFace::Text(rows);
            }
        }
        IdleFace::Hidden
    }

    pub fn on_event(&mut self, ev: PetEv) -> Vec<String> {
        if !self.live.enabled {
            return Vec::new();
        }
        let unlocked = self.live.on_event(ev, now());
        if self.live.needs_save {
            self.live.needs_save = false;
            self.persist_save();
        }
        unlocked
    }

    /// Drop the portrait so a full-screen layer is not covered. The offscreen
    /// sink becomes fully transparent. A real panel is painted black; the
    /// terminal clear that follows redraws the cells fbterm owns.
    pub fn conceal(&mut self) {
        let pixels = crate::fb::WIDTH * crate::fb::HEIGHT;
        let mut blank = Vec::with_capacity(pixels * 3);
        for _ in 0..pixels {
            blank.extend_from_slice(&crate::pet_art::KEY);
        }
        if let Some(path) = &self.out_path {
            let _ = crate::fb::write_ppm(path, &blank);
        }
        if let Some(panel) = self.panel.as_mut() {
            let _ = panel.fill([0, 0, 0]);
        }
    }

    pub fn present(&mut self) {
        let painted = if self.live.mode == pet::Mode::Pixel { self.live.paint(now()) } else { None };
        // A missing picture still replaces the offscreen sink, so a dock that
        // was just closed does not leave the previous girl on the next shot.
        let rgb = match painted {
            Some(buf) => buf,
            None if self.out_path.is_some() => {
                let mut blank = Vec::with_capacity(crate::fb::WIDTH * crate::fb::HEIGHT * 3);
                for _ in 0..(crate::fb::WIDTH * crate::fb::HEIGHT) {
                    blank.extend_from_slice(&[255, 0, 255]);
                }
                blank
            }
            None => return,
        };
        if let Some(path) = &self.out_path {
            let _ = crate::fb::write_ppm(path, &rgb);
        }
        if let Some(panel) = self.panel.as_mut() {
            let _ = panel.blit_rgb(&rgb);
        }
    }

    pub fn command(&mut self, arg: &str, cfg: &mut Config, persist: bool) -> Vec<(String, u8)> {
        let mut parts = arg.split_whitespace();
        let sub = parts.next().unwrap_or("").to_ascii_lowercase();
        let rest: Vec<&str> = parts.collect();
        let rest = rest.join(" ");
        let dim = 5u8;
        let err = 4u8;
        let norm = 0u8;
        let mut lines = Vec::new();
        let mut save_cfg = false;
        let mut save_pet = false;
        match sub.as_str() {
            "" => {
                lines.push((crate::i18n::t("pet_menu").to_string(), dim));
                lines.push((self.stats_line(), dim));
                lines.push((self.mode_line(), dim));
                if !self.live.pack_error.is_empty() {
                    lines.push((crate::i18n::tf("pet_pack_bad", &[&self.live.pack_error]), err));
                }
            }
            "on" => {
                cfg.pet_enabled = true;
                save_cfg = true;
                self.apply_cfg(cfg);
                self.tried = false;
                self.resolve();
                let unlocked = self.live.sign_in(now());
                save_pet = true;
                lines.push((crate::i18n::t("pet_on").to_string(), norm));
                lines.push((self.mode_line(), dim));
                self.push_unlocks(&mut lines, &unlocked);
            }
            "off" => {
                cfg.pet_enabled = false;
                save_cfg = true;
                self.apply_cfg(cfg);
                self.resolve();
                lines.push((crate::i18n::t("pet_off").to_string(), norm));
            }
            "dock" => {
                cfg.pet_dock = match rest.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => !cfg.pet_dock,
                };
                save_cfg = true;
                self.apply_cfg(cfg);
                self.resolve();
                lines.push((
                    crate::i18n::t(if cfg.pet_dock { "pet_dock_on" } else { "pet_dock_off" }).to_string(),
                    norm,
                ));
            }
            "room" => {
                cfg.pet_room = match rest.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => !cfg.pet_room,
                };
                save_cfg = true;
                self.apply_cfg(cfg);
                lines.push((
                    crate::i18n::t(if cfg.pet_room { "pet_room_on" } else { "pet_room_off" }).to_string(),
                    norm,
                ));
            }
            "text" | "pixel" | "auto" => {
                cfg.pet_render = sub.clone();
                save_cfg = true;
                self.apply_cfg(cfg);
                self.tried = false;
                self.resolve();
                lines.push((self.mode_line(), norm));
            }
            "pat" | "feed" | "praise" => {
                if !self.live.enabled {
                    lines.push((crate::i18n::t("pet_need_on").to_string(), err));
                } else {
                    let ev = match sub.as_str() {
                        "feed" => PetEv::Feed,
                        "praise" => PetEv::Praise,
                        _ => PetEv::Pat,
                    };
                    let key = match sub.as_str() {
                        "feed" => "pet_feed",
                        "praise" => "pet_praise",
                        _ => "pet_pat",
                    };
                    let unlocked = self.on_event(ev);
                    save_pet = true;
                    lines.push((crate::i18n::t(key).to_string(), norm));
                    self.push_unlocks(&mut lines, &unlocked);
                }
            }
            "stats" => lines.push((self.stats_line(), norm)),
            "export" => match self.export_to(rest.trim()) {
                Ok(path) => lines.push((crate::i18n::tf("pet_exported", &[&path]), norm)),
                Err(e) => lines.push((crate::i18n::tf("pet_import_bad", &[&e]), err)),
            },
            "import" => {
                let mut bits = rest.split_whitespace();
                let path = bits.next().unwrap_or("");
                let confirm = bits.next() == Some("confirm");
                if path.is_empty() {
                    lines.push((crate::i18n::t("cmd_pet_usage").to_string(), err));
                } else if !confirm {
                    lines.push((crate::i18n::tf("pet_import_confirm", &[path]), err));
                } else {
                    match self.import_from(Path::new(path)) {
                        Ok(unlocked) => {
                            save_pet = true;
                            lines.push((crate::i18n::t("pet_imported").to_string(), norm));
                            self.push_unlocks(&mut lines, &unlocked);
                        }
                        Err(e) => lines.push((crate::i18n::tf("pet_import_bad", &[&e]), err)),
                    }
                }
            }
            "pack" => {
                self.live.load_pack(&self.pack_dir);
                if self.live.pack_error.is_empty() {
                    let n = self.live.pack.as_ref().map(|p| if p.is_empty() { 0 } else { 1 }).unwrap_or(0);
                    let label = if n == 0 {
                        format!("{} (built-in)", self.pack_dir.display())
                    } else {
                        self.pack_dir.display().to_string()
                    };
                    lines.push((crate::i18n::tf("pet_pack_ok", &[&label]), norm));
                } else {
                    lines.push((crate::i18n::tf("pet_pack_bad", &[&self.live.pack_error]), err));
                }
            }
            "fb" => {
                cfg.pet_fb = if rest.trim().is_empty() || rest.trim() == "clear" {
                    String::new()
                } else {
                    rest.trim().to_string()
                };
                save_cfg = true;
                self.panel = None;
                self.tried = false;
                self.apply_cfg(cfg);
                self.resolve();
                lines.push((self.mode_line(), norm));
            }
            _ => lines.push((crate::i18n::t("cmd_pet_usage").to_string(), err)),
        }
        if persist && save_cfg {
            if let Err(e) = cfg.save() {
                lines.push((e, err));
            }
        }
        if persist && save_pet {
            self.persist_save();
            if !self.last_save_error.is_empty() {
                lines.push((crate::i18n::tf("pet_save_bad", &[&self.last_save_error]), err));
            }
        }
        lines
    }

    fn push_unlocks(&self, lines: &mut Vec<(String, u8)>, ids: &[String]) {
        for id in ids {
            let name = pet_growth::achievement_name(id);
            lines.push((crate::i18n::tf("pet_ach", &[&format!("{id} {name}")]), 5));
        }
    }

    fn stats_line(&self) -> String {
        let g = &self.live.save.growth;
        crate::i18n::tf(
            "pet_stats",
            &[
                &g.level.max(1).to_string(),
                &self.live.save.mood_pct().to_string(),
                &self.live.save.satiety_pct().to_string(),
                &self.live.save.bond_pct().to_string(),
            ],
        )
    }

    fn mode_line(&self) -> String {
        let which = match self.live.mode {
            pet::Mode::Off => crate::i18n::t("pet_off"),
            pet::Mode::Text => crate::i18n::t("pet_text"),
            pet::Mode::Pixel => crate::i18n::t("pet_pixel"),
        };
        if self.panel.is_some() {
            format!("{which}  {}", self.fb_override)
        } else if let Some(path) = &self.out_path {
            format!("{which}  {}", path.display())
        } else {
            which.to_string()
        }
    }

    fn export_to(&self, path: &str) -> Result<String, String> {
        let now = pet_growth::now_ms();
        let json = pet_growth::export_json(&self.live.save, now, "dst 0.2.0");
        let code = pet_growth::export_code(&self.live.save, now, "dst 0.2.0");
        let (json_path, code_path) = if path.is_empty() {
            (
                crate::config::home().join("pet-export.json"),
                crate::config::home().join("pet-export.wm1"),
            )
        } else {
            let p = PathBuf::from(path);
            let code_path = p.with_extension("wm1");
            (p, code_path)
        };
        if let Some(dir) = json_path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
        }
        std::fs::write(&json_path, json).map_err(|e| e.to_string())?;
        std::fs::write(&code_path, code).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&json_path, mode.clone());
            let _ = std::fs::set_permissions(&code_path, mode);
        }
        Ok(format!("{}  {}", json_path.display(), code_path.display()))
    }

    fn import_from(&mut self, path: &Path) -> Result<Vec<String>, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        pet_growth::reject_oversized(meta.len())?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut save = pet_growth::import_text(&text, pet_growth::now_ms())?;
        pet_growth::backup_save(&self.save_path)?;
        // Keep the device clock so a desktop export does not look like a
        // two-hour absence on the next tick.
        save.last_seen = pet_growth::now_ms();
        self.live.save = save;
        Ok(Vec::new())
    }
}

fn pack_dir_from_env() -> PathBuf {
    std::env::var("DST_PET_PACK")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join("pet-pack"))
}

fn load_preview_pack() -> Option<crate::pet_art::Pack> {
    let dir = pack_dir_from_env();
    match crate::pet_art::load_pack(&dir) {
        Ok(pack) => pack,
        Err(e) => {
            eprintln!("dst: pet pack: {e}");
            None
        }
    }
}

#[cfg(test)]
pub fn pixel_dock_for_test() -> Host {
    let mut host = Host {
        live: pet::Live::default(),
        panel: None,
        tried: true,
        fb_override: String::new(),
        out_path: Some(std::env::temp_dir().join("dst-pet-wrap-test.ppm")),
        pack_dir: std::env::temp_dir(),
        save_path: std::env::temp_dir().join("dst-pet-wrap-test.json"),
        last_save_error: String::new(),
    };
    host.live.configure(true, true, true, "pixel");
    host.resolve();
    host.follow(false);
    host
}

#[cfg(test)]
pub fn set_out_for_test(host: &mut Host, path: std::path::PathBuf) {
    host.out_path = Some(path);
}

fn now() -> u64 {
    pet_growth::now_ms().max(0) as u64
}

/// `dst --pet-preview <dock|room> <pose> <out.ppm>` or `sheet <dir>`.
/// Uses the same `render` as the live pet. No network, no framebuffer.
pub fn preview(args: &[String]) -> i32 {
    let kind = args.get(2).map(|s| s.as_str()).unwrap_or("sheet");
    if kind == "sheet" {
        let dir = PathBuf::from(args.get(3).map(|s| s.as_str()).unwrap_or("pet-preview"));
        if let Err(e) = write_sheet(&dir) {
            eprintln!("dst: {e}");
            return 1;
        }
        println!("{}", dir.display());
        return 0;
    }
    let pose = args.get(3).map(|s| s.as_str()).unwrap_or("idle");
    let out = args.get(4).cloned().unwrap_or_else(|| format!("pet-{kind}-{pose}.ppm"));
    let scene = if kind == "room" { crate::pet_art::Scene::Room } else { crate::pet_art::Scene::Dock };
    let paint = crate::pet_art::Paint::new(scene, pose, prop_for(pose));
    let pack = load_preview_pack();
    let rgb = crate::pet_art::render(&paint, pack.as_ref());
    if let Err(e) = crate::fb::write_ppm(Path::new(&out), &rgb) {
        eprintln!("dst: {e}");
        return 1;
    }
    println!("{out}");
    0
}

fn prop_for(pose: &str) -> &'static str {
    match pose {
        "tool" | "shell" => "shell",
        "read" => "read",
        "web" => "web",
        "write" => "write",
        "think" | "plan" => "plan",
        _ => "",
    }
}

fn write_sheet(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let pack = load_preview_pack();
    for pose in ["idle", "blink", "think", "talk", "happy", "sad", "eat", "blush", "sleep", "tool"] {
        let paint = crate::pet_art::Paint::new(crate::pet_art::Scene::Dock, pose, prop_for(pose));
        let rgb = crate::pet_art::render(&paint, pack.as_ref());
        crate::fb::write_ppm(&dir.join(format!("dock-{pose}.ppm")), &rgb)?;
    }
    for pose in ["idle", "sleep", "happy"] {
        let paint = crate::pet_art::Paint::new(crate::pet_art::Scene::Room, pose, "");
        let rgb = crate::pet_art::render(&paint, pack.as_ref());
        crate::fb::write_ppm(&dir.join(format!("room-{pose}.ppm")), &rgb)?;
    }
    let text_path = dir.join("text-idle.txt");
    let rows = crate::pet_art::text_rows("idle");
    std::fs::write(&text_path, rows.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_commands_do_not_persist() {
        let mut cfg = Config::default();
        let mut host = Host::boot(&cfg);
        assert!(!host.live.enabled);
        assert_eq!(host.dock_width(53), 0);
        let lines = host.command("on", &mut cfg, false);
        assert!(cfg.pet_enabled);
        assert!(lines.iter().any(|(t, _)| t.contains("whale") || !t.is_empty()));
        host.out_path = Some(std::env::temp_dir().join(format!("dst-pet-preview-{}.ppm", std::process::id())));
        host.resolve();
        assert_eq!(host.live.mode, pet::Mode::Pixel);
        host.follow(false);
        assert_eq!(host.dock_width(53), 43);
        host.command("off", &mut cfg, false);
        assert!(!cfg.pet_enabled);
        assert_eq!(host.dock_width(53), 0);
        if let Some(p) = &host.out_path {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn import_waits_for_confirm_then_backs_the_old_save_up() {
        let dir = std::env::temp_dir().join(format!("dst-pet-import-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut cfg = Config::default();
        let mut host = Host {
            live: pet::Live::default(),
            panel: None,
            tried: true,
            fb_override: String::new(),
            out_path: None,
            pack_dir: dir.join("pack"),
            save_path: dir.join("pet.json"),
            last_save_error: String::new(),
        };
        host.live.configure(true, false, false, "text");
        host.live.save.title = "old".into();
        pet_growth::store_file(&host.save_path, &host.live.save).unwrap();
        let mut incoming = pet_growth::Save::default();
        incoming.title = "Task-master".into();
        let src = dir.join("in.json");
        std::fs::write(&src, pet_growth::export_json(&incoming, 1_763_600_000_000, "ref")).unwrap();
        let preview = host.command(&format!("import {}", src.display()), &mut cfg, false);
        assert!(preview.iter().any(|(t, _)| t.contains("confirm")), "{preview:?}");
        assert_eq!(host.live.save.title, "old");
        let applied = host.command(&format!("import {} confirm", src.display()), &mut cfg, true);
        assert!(applied.iter().any(|(t, _)| !t.is_empty()));
        assert_eq!(host.live.save.title, "Task-master");
        assert_eq!(std::fs::read_to_string(dir.join("pet.json.bak")).unwrap().contains("old"), true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pat_is_still_there_after_reload_and_a_write_error_is_shown() {
        let dir = std::env::temp_dir().join(format!("dst-pet-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        let mut host = Host {
            live: pet::Live::default(),
            panel: None,
            tried: true,
            fb_override: String::new(),
            out_path: None,
            pack_dir: dir.join("pack"),
            save_path: dir.join("pet.json"),
            last_save_error: String::new(),
        };
        host.live.configure(true, false, false, "text");
        let lines = host.command("pat", &mut cfg, true);
        assert!(lines.iter().any(|(t, _)| !t.is_empty()));
        let loaded = pet_growth::load_file(&host.save_path);
        assert_eq!(loaded.usage.pats, 1, "the pat counter has to survive a reload");
        let blocker = dir.join("blocked");
        std::fs::write(&blocker, "x").unwrap();
        host.save_path = blocker.join("pet.json");
        let failed = host.command("pat", &mut cfg, true);
        assert!(
            failed.iter().any(|(t, _)| t.contains("could not save") || t.contains("保存")),
            "a failed save must be visible, got {failed:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
