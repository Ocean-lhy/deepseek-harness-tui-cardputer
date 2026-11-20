//! Human commands — the `/` commands, modelled on DSH's `dsh-commands`
//! registry: a command has a name, a description, an optional free-form input
//! hint, and a handler that runs immediately without creating a model message.
//!
//! Results are UI outcomes (transcript lines), never tool results.

use crate::config::{self, Config};

pub const KIND_NORMAL: u8 = 0;
#[allow(dead_code)]
pub const KIND_TOOL: u8 = 2;
pub const KIND_ERR: u8 = 4;
pub const KIND_DIM: u8 = 5;

/// One registered command, mirroring DSH's `CommandDefinition`.
pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
    /// Placeholder advertised before free-form input, e.g. `[name]`.
    pub input_hint: Option<&'static str>,
    /// Grouping key used by the palette/help (`cat_*` i18n keys).
    pub category: &'static str,
}

/// Category display order for the palette and help menu.
pub fn category_rank(cat: &str) -> u8 {
    match cat {
        "cat_session" => 0,
        "cat_goal" => 1,
        "cat_core" => 2,
        "cat_setup" => 3,
        "cat_tools" => 4,
        _ => 5,
    }
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "help",
        category: "cat_core",
        description: "list commands",
        input_hint: None,
    },
    Command {
        name: "model",
        category: "cat_core",
        description: "show or set the model",
        input_hint: Some("[name]"),
    },
    Command {
        name: "effort",
        category: "cat_core",
        description: "show or set reasoning effort (auto|low|high|max)",
        input_hint: Some("[auto|low|high|max]"),
    },
    Command {
        name: "login",
        category: "cat_setup",
        description: "configure the API key by scanning a QR code",
        input_hint: None,
    },
    Command {
        name: "key",
        category: "cat_setup",
        description: "set the API key directly",
        input_hint: Some("<api-key>"),
    },
    Command {
        name: "usage",
        category: "cat_info",
        description: "token usage, cache hit rate and context pressure",
        input_hint: None,
    },
    Command {
        name: "btw",
        category: "cat_info",
        description: "ask a one-off read-only subagent a side question (answer in a panel)",
        input_hint: Some("<question>"),
    },
    Command {
        name: "image",
        category: "cat_input",
        description: "attach a local image (JPEG/PNG/GIF/WebP) to the next message",
        input_hint: Some("[<path>|clear]"),
    },
    Command {
        name: "update",
        category: "cat_setup",
        description: "check the update channel and install a newer dst build (nightly|stable)",
        input_hint: Some("[nightly|stable]"),
    },
    Command {
        name: "update-hwinfo",
        category: "cat_info",
        description: "rescan this device's hardware and refresh the hardware-info skill",
        input_hint: None,
    },
    Command {
        name: "compact",
        category: "cat_session",
        description: "summarise older history now (same method as automatic compaction)",
        input_hint: None,
    },
    Command {
        name: "goal",
        category: "cat_goal",
        description: "show or manage the session goal (long-running objective)",
        input_hint: Some("[show|<objective>|edit <objective>|pause|resume|clear]"),
    },
    Command {
        name: "resume",
        category: "cat_session",
        description: "pick a previous session from a menu (expand to preview)",
        input_hint: None,
    },
    Command {
        name: "rewind",
        category: "cat_session",
        description: "restore a checkpoint from an earlier turn (menu picker)",
        input_hint: None,
    },
    Command {
        name: "extensions",
        category: "cat_tools",
        description: "inspect skills and MCP servers (menu)",
        input_hint: None,
    },
    Command {
        name: "rename",
        category: "cat_session",
        description: "name the current saved session (shown in /resume)",
        input_hint: Some("<title>"),
    },
    Command {
        name: "config",
        category: "cat_core",
        description: "interactive configuration (options first)",
        input_hint: Some("[wizard]"),
    },
    Command {
        name: "apps",
        category: "cat_tools",
        description: "browse, run or delete the apps dst built (menu)",
        input_hint: Some("[run <name>]"),
    },
    Command {
        name: "clear",
        category: "cat_session",
        description: "clear the transcript",
        input_hint: None,
    },
    Command {
        name: "new",
        category: "cat_session",
        description: "start a new conversation (same as /clear)",
        input_hint: None,
    },
    Command {
        name: "quit",
        category: "cat_core",
        description: "exit dst",
        input_hint: None,
    },
    Command {
        name: "pet",
        category: "cat_core",
        description: "whale-girl desk pet",
        input_hint: Some("[on|off|dock|room|pat|feed|praise|stats|export|import|pack]"),
    },
];

/// Commands matching what has been typed after `/` (before any space).
/// Used by the interactive palette.
/// Localized one-line description for a command.
pub fn describe(name: &str) -> String {
    let key = format!("cmd_{name}");
    let t = crate::i18n::t(&key);
    if t.is_empty() {
        COMMANDS
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.description.to_string())
            .unwrap_or_default()
    } else {
        t.to_string()
    }
}

pub fn palette(input: &str) -> Vec<(&'static str, String, Option<&'static str>, &'static str)> {
    let t = input.trim_start();
    if !t.starts_with('/') {
        return Vec::new();
    }
    let q = &t[1..];
    if q.contains(char::is_whitespace) {
        return Vec::new();
    }
    let ql = q.to_lowercase();
    let mut items: Vec<&Command> = COMMANDS
        .iter()
        .filter(|c| {
            q.is_empty()
                || c.name.starts_with(q)
                || describe(c.name).to_lowercase().contains(&ql)
        })
        .collect();
    // Name matches outrank description matches: typing "/resume" in Chinese
    // also matched /rename (its description mentions /resume), and the
    // alphabetical first row made Enter complete /rename instead.
    items.sort_by(|a, b| {
        let an = q.is_empty() || a.name.starts_with(q);
        let bn = q.is_empty() || b.name.starts_with(q);
        bn.cmp(&an)
            .then_with(|| category_rank(a.category).cmp(&category_rank(b.category)))
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.cmp(b.name))
    });
    items
        .into_iter()
        .map(|c| (c.name, describe(c.name), c.input_hint, c.category))
        .collect()
}

/// How a `/` command may run while a model turn is already in flight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BusyPolicy {
    /// Run right away: no model call and no state the running turn relies on.
    Immediate,
    /// Needs a stopped session: changes the model/endpoint/history or takes
    /// over the screen. Refused instead of queued (only chat input queues).
    Refuse,
}

/// Commands that may run in the middle of a turn. Everything else is refused
/// with a notice: commands are never queued as if they were chat messages.
pub const IMMEDIATE_WHILE_BUSY: &[&str] = &[
    "usage",
    "btw",
    "image",
    "rename",
    "update-hwinfo",
    "goal",
    "quit",
    "pet",
];

/// Canonical command name for a typed `/` line, aliases included. `None` when
/// the line is not a command.
pub fn command_name(input: &str) -> Option<&'static str> {
    let body = input.trim().strip_prefix('/')?;
    let raw = body.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    let canonical = match raw.as_str() {
        "?" => "help",
        "img" | "photo" => "image",
        "update_hwinfo" | "hwinfo" => "update-hwinfo",
        "exit" | "q" => "quit",
        "new" => "clear",
        other => other,
    };
    COMMANDS
        .iter()
        .find(|c| c.name == canonical)
        .map(|c| c.name)
}

/// Classify the line the user pressed Enter on while the model is busy.
pub fn busy_policy(input: &str) -> BusyPolicy {
    match command_name(input) {
        Some(name) if IMMEDIATE_WHILE_BUSY.contains(&name) => BusyPolicy::Immediate,
        _ => BusyPolicy::Refuse,
    }
}

pub struct Outcome {
    pub lines: Vec<(String, u8)>,
    pub quit: bool,
    pub clear: bool,
    /// Start the QR login server (main owns the UI and the config write).
    pub login: bool,
    /// Open the interactive configuration wizard.
    pub config_wizard: bool,
    /// Open the menuconfig-style /resume picker (main owns the screen).
    pub resume_menu: bool,
    /// Open the model picker menu (main owns /v1/models fetch and the screen).
    pub model_menu: bool,
    /// Run the manual compaction flow (main owns the transcript).
    pub compact: bool,
    /// Open the checkpoint picker (main owns the transcript).
    pub rewind_menu: bool,
    /// Open the skills/MCP inspection menu.
    pub extensions_menu: bool,
    /// Open the command palette menu (/help).
    pub help_menu: bool,
    /// Open the saved-app manager (creator mode /apps).
    pub apps_menu: bool,
    /// Run this saved app on the screen after the command handling (/apps run).
    pub run_app: Option<String>,
    /// Seed the next model turn with this request (a command that hands work
    /// to the model, e.g. /update-hwinfo refining the hardware skill).
    pub model_prompt: Option<String>,
    /// Attach this local image file to the next message (/image <path>).
    pub image: Option<String>,
    /// Drop every pending image attachment (/image clear).
    pub image_clear: bool,
    /// Ask a one-off read-only subagent this side question (/btw <question>).
    pub btw: Option<String>,
    /// Check for a self-update; `Some(channel)` switches the channel first,
    /// `Some("")` keeps the configured one (/update [nightly|stable]).
    pub update: Option<String>,
    /// Raw `/pet` argument. Empty string opens the pet menu.
    pub pet: Option<String>,
}

impl Outcome {
    fn text(s: impl Into<String>, kind: u8) -> Outcome {
        Outcome {
            lines: vec![(s.into(), kind)],
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
            compact: false,
        }
    }
    fn many(lines: Vec<(String, u8)>) -> Outcome {
        Outcome {
            lines,
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
            compact: false,
        }
    }
}


fn usage(prefix: &str) -> Outcome {
    let mut lines = vec![(format!("{prefix}"), KIND_ERR)];
    lines.push(("try /help".into(), KIND_DIM));
    Outcome::many(lines)
}

/// Dispatch a `/` command. Returns `None` when the input is not a command.
pub fn dispatch(input: &str, cfg: &mut Config) -> Option<Outcome> {
    evaluate(input, cfg, true)
}

/// `persist=false` keeps the state change in memory only (tests).
pub fn evaluate(input: &str, cfg: &mut Config, persist: bool) -> Option<Outcome> {
    let t = input.trim();
    if !t.starts_with('/') {
        return None;
    }
    let body = &t[1..];
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    let arg = parts.next().unwrap_or("").trim();
    let name = name.to_ascii_lowercase();

    let out = match name.as_str() {
        "pet" => {
            let mut o = Outcome::text("", KIND_DIM);
            o.lines.clear();
            o.pet = Some(arg.to_string());
            o
        }
        "help" | "?" => Outcome {
            lines: Vec::new(),
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            compact: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: true,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
        },
        "model" => {
            if arg.is_empty() {
                Outcome {
                    lines: Vec::new(),
                    quit: false,
                    clear: false,
                    login: false,
                    config_wizard: false,
                    resume_menu: false,
                    model_menu: true,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
                    compact: false,
                }
            } else if arg.len() > 120 {
                usage("/model <name>")
            } else {
                cfg.model = arg.to_string();
                let saved = if persist { cfg.save() } else { Ok(()) };
                match saved {
                    Ok(()) => Outcome::text(crate::i18n::tf("cmd_model_set", &[&cfg.model]), KIND_NORMAL),
                    Err(e) => Outcome::text(crate::i18n::tf("cmd_model_save_failed", &[&e.to_string()]), KIND_ERR),
                }
            }
        }
        "effort" => {
            if arg.is_empty() {
                Outcome::many(vec![
                    (format!("effort  {}", cfg.effort), KIND_NORMAL),
                    (crate::i18n::t("effort_values").to_string(), KIND_DIM),
                    ("usage: /effort high".into(), KIND_DIM),
                ])
            } else {
                let v = arg.to_ascii_lowercase();
                if !matches!(v.as_str(), "auto" | "low" | "high" | "max") {
                    usage("/effort [auto|low|high|max]")
                } else {
                    cfg.effort = v.clone();
                    let saved = if persist { cfg.save() } else { Ok(()) };
                    match saved {
                        Ok(()) => Outcome::text(crate::i18n::tf("cmd_effort_set", &[&v]), KIND_NORMAL),
                        Err(e) => Outcome::text(crate::i18n::tf("cmd_effort_save_failed", &[&e.to_string()]), KIND_ERR),
                    }
                }
            }
        }
        "key" => {
            if arg.is_empty() {
                usage("/key <api-key>")
            } else {
                // Never echo the key back.
                cfg.api_key = arg.to_string();
                cfg.dsh_token.clear();
                cfg.configured = true;
                let saved = if persist { cfg.save() } else { Ok(()) };
                match saved {
                    Ok(()) => Outcome::text(
                        crate::i18n::tf("cmd_key_saved", &[&config::key_path().display().to_string()]),
                        KIND_NORMAL,
                    ),
                    Err(e) => Outcome::text(crate::i18n::tf("cmd_key_save_failed", &[&e.to_string()]), KIND_ERR),
                }
            }
        }
        "login" => {
            let mut o = login_hint(cfg);
            o.login = true;
            o
        }
        "config" => {
            if arg == "path" {
                Outcome::text(config::config_path().display().to_string(), KIND_NORMAL)
            } else if arg.is_empty() || arg == "wizard" {
                let mut o = Outcome::many(vec![
                    ("打开配置向导…".to_string(), KIND_DIM),
                ]);
                o.config_wizard = true;
                o
            } else {
                let mut lines = vec![(crate::i18n::t("cmd_config_effective").to_string(), KIND_DIM)];
                for l in cfg.summary() {
                    lines.push((l, KIND_NORMAL));
                }
                lines.push((crate::i18n::t("cmd_config_edit_hint").to_string(), KIND_DIM));
                Outcome::many(lines)
            }
        }
        "btw" => {
            if arg.trim().is_empty() {
                Outcome::many(vec![(crate::i18n::t("cmd_btw_usage").to_string(), KIND_DIM)])
            } else {
                let mut out = Outcome::text("", KIND_DIM);
                out.lines.clear();
                out.btw = Some(arg.trim().to_string());
                out
            }
        }
        "image" | "img" | "photo" => {
            let arg = arg.trim();
            if arg.eq_ignore_ascii_case("clear") {
                let mut out = Outcome::text("", KIND_DIM);
                out.lines.clear();
                out.image_clear = true;
                out
            } else if arg.is_empty() {
                Outcome::many(vec![(
                    crate::i18n::t("cmd_image_usage").to_string(),
                    KIND_DIM,
                )])
            } else {
                let mut out = Outcome::text("", KIND_DIM);
                out.lines.clear();
                out.image = Some(arg.trim_matches(|c| c == '"' || c == '\'').to_string());
                out
            }
        }
        "update" => {
            let arg = arg.trim().to_ascii_lowercase();
            if !arg.is_empty() && crate::update::Channel::parse(&arg).is_none() {
                Outcome::many(vec![
                    (crate::i18n::t("cmd_update_usage").to_string(), KIND_ERR),
                    (crate::i18n::t("try_help").to_string(), KIND_DIM),
                ])
            } else {
                let mut out = Outcome::text("", KIND_DIM);
                out.lines.clear();
                out.update = Some(arg);
                out
            }
        }
        "update-hwinfo" | "update_hwinfo" | "hwinfo" => {
            // The scan is local and deterministic: it works in any mode. In
            // creator mode the model then reads the refreshed skill and refines
            // the parts that need judgement.
            match crate::hwinfo::update() {
                Ok(s) => {
                    let mut out = Outcome::many(vec![(
                        crate::i18n::tf(
                            "hw_scanned",
                            &[
                                &s.sections.to_string(),
                                &(s.bytes / 1024).max(1).to_string(),
                                &s.version.to_string(),
                            ],
                        ),
                        KIND_DIM,
                    )]);
                    if cfg.creator {
                        out.model_prompt = Some(crate::hwinfo::refine_prompt(&s));
                    } else {
                        out.lines.push((
                            crate::i18n::t("hw_refine_hint").to_string(),
                            KIND_DIM,
                        ));
                    }
                    out
                }
                Err(e) => Outcome::many(vec![(
                    crate::i18n::tf("hw_scan_failed", &[&e]),
                    KIND_ERR,
                )]),
            }
        }
        "usage" => {
            // `report()` already opens with "<usage> · N requests"; a separate
            // heading row duplicated it and cost one of the 12 panel rows.
            let mut lines = Vec::new();
            for l in crate::usage::report() {
                lines.push((l, KIND_NORMAL));
            }
            if !config::is_official(&cfg.base) {
                lines.push((crate::i18n::t("cmd_usage_note").to_string(), KIND_DIM));
            }
            Outcome::many(lines)
        }
        "resume" => {
            if config::list_sessions().is_empty() {
                Outcome::many(vec![(crate::i18n::t("resume_empty").to_string(), KIND_DIM)])
            } else {
                Outcome {
                    lines: Vec::new(),
                    quit: false,
                    clear: false,
                    login: false,
                    config_wizard: false,
                    resume_menu: true,
                    model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
                    compact: false,
                }
            }
        }
        "rewind" => Outcome {
            lines: vec![(crate::i18n::t("cmd_rewind_pick").to_string(), KIND_DIM)],
            quit: false, clear: false, login: false, config_wizard: false,
            resume_menu: false, model_menu: false, compact: false,
            rewind_menu: true, extensions_menu: false, help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
        },
        "extensions" => Outcome {
            lines: Vec::new(), quit: false, clear: false, login: false,
            config_wizard: false, resume_menu: false, model_menu: false,
            compact: false, rewind_menu: false, extensions_menu: true, help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
        },
        "rename" => {
            if arg.is_empty() {
                usage("/rename <title>")
            } else {
                match crate::session::rename(arg) {
                    Ok(title) => Outcome::text(crate::i18n::tf("cmd_rename_ok", &[&title]), KIND_NORMAL),
                    Err(e) => Outcome::text(crate::i18n::tf("cmd_rename_failed", &[&e.to_string()]), KIND_ERR),
                }
            }
        }
        "compact" => Outcome {
            lines: vec![(crate::i18n::t("cmd_compact_progress").to_string(), KIND_DIM)],
            quit: false,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
            compact: true,
        },
        "goal" => {
            let lower = arg.to_ascii_lowercase();
            let result = if arg.is_empty() || lower == "show" || lower == "status" {
                Ok(crate::goal::status_lines())
            } else if lower == "clear" {
                match crate::goal::clear() {
                    Ok(()) => Ok(vec![crate::i18n::t("goal_cleared").to_string()]),
                    Err(e) => Err(e),
                }
            } else if lower == "pause" {
                let g = crate::goal::load().ok_or(crate::i18n::t("goal_no_goal").to_string());
                g.and_then(|g| crate::goal::pause(&g.id, g.revision))
                    .map(|g| {
                        let mut l = vec![crate::i18n::t("goal_paused").to_string()];
                        l.extend(crate::goal::status_lines().into_iter().skip(1));
                        let _ = g;
                        l
                    })
            } else if lower == "resume" {
                let g = crate::goal::load().ok_or(crate::i18n::t("goal_no_goal").to_string());
                g.and_then(|g| crate::goal::resume(&g.id, g.revision))
                    .map(|_| {
                        let mut l = vec![crate::i18n::t("goal_resumed").to_string()];
                        l.extend(crate::goal::status_lines().into_iter().skip(1));
                        l
                    })
            } else if lower == "edit" {
                Err(crate::i18n::t("goal_usage").to_string())
            } else if lower.starts_with("edit ") || lower.starts_with("edit\t") {
                let objective = arg[4..].trim().to_string();
                let g = crate::goal::load().ok_or(crate::i18n::t("goal_no_goal").to_string());
                g.and_then(|g| crate::goal::edit(&g.id, g.revision, Some(&objective), None))
                    .map(|_| {
                        let mut l = vec![crate::i18n::t("goal_updated").to_string()];
                        l.extend(crate::goal::status_lines().into_iter().skip(1));
                        l
                    })
            } else {
                crate::goal::create(arg, None).map(|_| {
                    let mut l = vec![crate::i18n::t("goal_created").to_string()];
                    l.extend(crate::goal::status_lines().into_iter().skip(1));
                    l
                })
            };
            match result {
                Ok(lines) => Outcome::many(lines.into_iter().map(|l| (l, KIND_NORMAL)).collect()),
                Err(e) => Outcome::many(vec![(e, KIND_ERR)]),
            }
        }
        "apps" => {
            let apps = crate::apps::load();
            // `/apps run <name>` runs one app directly; bare `/apps` opens the menu.
            if let Some(rest) = arg.strip_prefix("run ").or_else(|| arg.strip_prefix("run\t")) {
                let name = rest.trim();
                if name.is_empty() {
                    usage("/apps run <name>")
                } else {
                    match crate::apps::find(name) {
                        None => Outcome::many(vec![(
                            crate::i18n::tf("apps_not_found", &[name]),
                            KIND_ERR,
                        )]),
                        Some(app) => Outcome {
                            lines: vec![(crate::i18n::tf("apps_launching", &[&app.label()]), KIND_DIM)],
                            quit: false,
                            clear: false,
                            login: false,
                            config_wizard: false,
                            resume_menu: false,
                            model_menu: false,
                            rewind_menu: false,
                            extensions_menu: false,
                            help_menu: false,
                            apps_menu: false,
                            run_app: Some(app.label()),
                            model_prompt: None,
                            image: None,
                            image_clear: false,
                            btw: None,
                            update: None,
                            pet: None,
                            compact: false,
                        },
                    }
                }
            } else if apps.is_empty() && !cfg.creator {
                Outcome::many(vec![(
                    crate::i18n::t("cmd_apps_empty").to_string(),
                    KIND_DIM,
                )])
            } else {
                Outcome {
                    lines: Vec::new(),
                    quit: false,
                    clear: false,
                    login: false,
                    config_wizard: false,
                    resume_menu: false,
                    model_menu: false,
                    rewind_menu: false,
                    extensions_menu: false,
                    help_menu: false,
                    apps_menu: true,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
                    compact: false,
                }
            }
        }
        "clear" | "new" => Outcome {
            lines: Vec::new(),
            quit: false,
            clear: true,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
            compact: false,
        },
        "quit" | "exit" | "q" => Outcome {
            lines: Vec::new(),
            quit: true,
            clear: false,
            login: false,
            config_wizard: false,
            resume_menu: false,
            model_menu: false,
            rewind_menu: false,
            extensions_menu: false,
            help_menu: false,
            apps_menu: false,
            run_app: None,
            model_prompt: None,
            image: None,
            image_clear: false,
            btw: None,
            update: None,
            pet: None,
            compact: false,
        },
        "" => usage("/"),
        other => Outcome::many(vec![
            (crate::i18n::tf("cmd_unknown", &[other]), KIND_ERR),
            (crate::i18n::t("try_help").to_string(), KIND_DIM),
        ]),
    };
    Some(out)
}

/// `/login`: when the key is missing this starts the guided setup; when a key
/// is present it shows how to replace it.
fn login_hint(cfg: &mut Config) -> Outcome {
    let mut lines = vec![("login".to_string(), KIND_DIM)];
    if !cfg.dsh_token.is_empty() {
        lines.push((crate::i18n::t("login_hint_token").to_string(), KIND_NORMAL));
        lines.push((crate::i18n::t("login_hint_token_replace").to_string(), KIND_DIM));
        lines.push((crate::i18n::t("login_hint_token_switch").to_string(), KIND_DIM));
    } else if cfg.configured {
        lines.push((crate::i18n::t("login_hint_key").to_string(), KIND_NORMAL));
        lines.push((crate::i18n::t("login_hint_key_replace").to_string(), KIND_DIM));
        lines.push((crate::i18n::t("login_hint_key_setup").to_string(), KIND_DIM));
    } else {
        lines.push((crate::i18n::t("login_hint_none").to_string(), KIND_ERR));
        lines.push((crate::i18n::t("login_hint_create").to_string(), KIND_NORMAL));
        lines.push((crate::i18n::t("login_hint_qr").to_string(), KIND_DIM));
    }
    Outcome::many(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn setting_an_api_key_clears_the_dsh_token() {
        let mut c = cfg();
        c.dsh_token = "dsh-token".into();
        c.api_key.clear();
        let _ = evaluate("/key sk-test", &mut c, false);
        assert_eq!(c.api_key, "sk-test");
        assert!(c.dsh_token.is_empty());
    }

    #[test]
    fn non_command_passes_through() {
        assert!(dispatch("hello", &mut cfg()).is_none());
    }

    #[test]
    fn model_sets_value() {
        let mut c = cfg();
        let o = evaluate("/model deepseek-v4", &mut c, false).unwrap();
        assert_eq!(c.model, "deepseek-v4");
        assert!(o.lines[0].0.contains("deepseek-v4"));
    }

    #[test]
    fn effort_validates() {
        let mut c = cfg();
        let bad = evaluate("/effort banana", &mut c, false).unwrap();
        assert!(bad.lines[0].0.starts_with("/effort"), "usage hint: {:?}", bad.lines);
        assert_eq!(c.effort, "auto");
        assert!(evaluate("/effort high", &mut c, false).is_some());
        assert_eq!(c.effort, "high");
    }

    #[test]
    fn key_is_never_echoed() {
        let mut c = cfg();
        let o = evaluate("/key sk-secret-value", &mut c, false).unwrap();
        let all: String = o.lines.iter().map(|(s, _)| s.clone()).collect();
        assert!(!all.contains("sk-secret-value"));
        assert_eq!(c.api_key, "sk-secret-value");
    }

    #[test]
    fn palette_prefers_name_matches() {
        // In Chinese, /rename's description mentions /resume, so both match.
        // The name match must sort first or Enter completes /rename instead.
        crate::i18n::set("zh");
        let pal = palette("/resume");
        let names: Vec<&str> = pal.iter().map(|p| p.0).collect();
        crate::i18n::set("en");
        assert_eq!(names.first().copied(), Some("resume"), "order: {names:?}");
        assert!(names.contains(&"rename"), "rename still matches by description");
    }

    #[test]
    fn quit_and_clear_flags() {
        assert!(dispatch("/quit", &mut cfg()).unwrap().quit);
        assert!(dispatch("/clear", &mut cfg()).unwrap().clear);
    }

    #[test]
    fn apps_command_and_mode_location() {
        let mut c = cfg();
        // Creator mode is a /config row, not a top-level command.
        assert!(!COMMANDS.iter().any(|c| c.name == "creator"));
        let out = evaluate("/creator", &mut c, false).unwrap();
        assert!(!c.creator, "there is no /creator command to toggle the mode");
        assert!(out.lines[0].1 == KIND_ERR, "{:?}", out.lines[0]);
        // No apps in this process: the empty state or the menu, never a panic.
        let out = evaluate("/apps", &mut c, false).unwrap();
        assert!(out.apps_menu || !out.lines.is_empty());
        let out = evaluate("/apps run nope", &mut c, false).unwrap();
        assert!(out.run_app.is_none());
        assert!(out.lines[0].1 == KIND_ERR, "{:?}", out.lines[0]);
        assert!(COMMANDS.iter().any(|c| c.name == "apps"));
        assert_eq!(palette("/app")[0].0, "apps");
        // In creator mode even an empty /apps opens the launcher so the
        // "+ new app" row (and the app-building flow) is reachable.
        c.creator = true;
        let out = evaluate("/apps", &mut c, false).unwrap();
        assert!(out.apps_menu, "creator mode opens the launcher");
    }

    #[test]
    fn busy_commands_are_classified() {
        // Safe to run mid-turn: no model turn, no history/model mutation.
        for line in [
            "/usage",
            "/btw what is in notes.txt",
            "/img ~/photo.png",
            "/image clear",
            "/rename a better title",
            "/update-hwinfo",
            "/goal pause",
            "/quit",
        ] {
            assert_eq!(busy_policy(line), BusyPolicy::Immediate, "{line}");
        }
        // Must wait for the turn to stop: model/endpoint changes, history,
        // screen-owning menus.
        for line in [
            "/model deepseek-v4-pro",
            "/effort high",
            "/config",
            "/key sk-test",
            "/login",
            "/clear",
            "/compact",
            "/resume",
            "/rewind",
            "/apps",
            "/extensions",
            "/help",
            "/not-a-command",
        ] {
            assert_eq!(busy_policy(line), BusyPolicy::Refuse, "{line}");
        }
        assert_eq!(command_name("not a command"), None);
        assert_eq!(command_name("/IMG"), Some("image"));
        assert_eq!(command_name("/q"), Some("quit"));
        // Every immediate command must actually be registered and dispatchable.
        for name in IMMEDIATE_WHILE_BUSY {
            assert!(
                COMMANDS.iter().any(|c| c.name == *name),
                "{name} is not registered"
            );
            let mut cfg = cfg();
            assert!(
                evaluate(&format!("/{name}"), &mut cfg, false).is_some(),
                "{name} does not dispatch"
            );
        }
    }
}
