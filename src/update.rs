//! Self-update from GitHub releases (DSH-style: rolling nightly or stable).
//!
//! Two channels, selected in `~/.dst/config.yml` (`update.channel`, default
//! `nightly`):
//!   * `nightly` — the rolling `nightly` prerelease, replaced on every commit to
//!     `main`.
//!   * `stable` — the latest official release (`releases/latest`).
//!
//! Both channels detect updates by comparing the published asset's SHA-256 in
//! `SHA256SUMS` against the running binary's SHA-256 (never by version string),
//! so switching channels or rebuilding the same version always converges.
//!
//! No crates and no HTTP client dependency: metadata and the binary come from
//! `curl`, hashes are computed in-process (SHA-256), and replacement is atomic
//! (write `.dst.update.<pid>` next to the binary, then rename over it),
//! followed by re-exec so the new build is running immediately.
//!
//! The startup check runs on a background thread and only *offers* an update;
//! `/update` checks and installs on demand.

use std::path::Path;
use std::sync::Mutex;

/// Repository that publishes the release assets. Override base (tests/local
/// mirrors) with `DST_UPDATE_BASE`.
pub const DEFAULT_BASE: &str =
    "https://github.com/MarsTechHAN/deepseek-harness-tui-cardputer";

pub const USER_AGENT: &str = "dst-updater";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Nightly,
    Stable,
}

impl Channel {
    pub fn parse(text: &str) -> Option<Channel> {
        match text.trim().to_ascii_lowercase().as_str() {
            "nightly" => Some(Channel::Nightly),
            "stable" => Some(Channel::Stable),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Nightly => "nightly",
            Channel::Stable => "stable",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub channel: Channel,
    /// Release tag label for stable (`v0.2.0` / `stable`), or `nightly`.
    pub version: String,
    pub asset: String,
    pub url: String,
    pub sha256: String,
    /// SHA-256 of the running binary when the check ran.
    pub local_sha256: String,
}

fn base() -> String {
    std::env::var("DST_UPDATE_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Release asset for this platform (None: self-update is unsupported here).
pub fn asset_name() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Some("dst-linux-aarch64"),
        "x86_64" => Some("dst-linux-x86_64"),
        _ => None,
    }
}

fn curl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "30", "-A", USER_AGENT])
        .args(args)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl failed: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn final_url(url: &str) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "--max-time",
            "30",
            "-A",
            USER_AGENT,
        ])
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl failed: {}", err.trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err("no redirect target from GitHub".into());
    }
    Ok(text)
}

/// `v0.2.0` / `0.2.0` -> the normalized version label.
pub fn tag_version(tag: &str) -> Option<String> {
    let v = tag.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    if v.is_empty() {
        return None;
    }
    let ok = v
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Some(v.to_string())
    } else {
        None
    }
}

/// True when the published asset's SHA-256 differs from the running binary's.
pub fn needs_update(local_sha256: &str, remote_sha256: &str) -> bool {
    !local_sha256.eq_ignore_ascii_case(remote_sha256)
}

/// Short hex prefix for UI display (8 chars).
pub fn short_sha(sha: &str) -> &str {
    let end = sha
        .char_indices()
        .nth(8)
        .map(|(i, _)| i)
        .unwrap_or(sha.len());
    &sha[..end]
}

/// `SHA256SUMS` text -> map of file name -> hex digest.
pub fn parse_sha256sums(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(sha) = fields.next() else { continue };
        let Some(name) = fields.next() else { continue };
        let name = name.trim_start_matches('*');
        if sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            out.insert(name.to_string(), sha.to_ascii_lowercase());
        }
    }
    out
}

fn hex_lower(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// SHA-256 of a file, computed in-process (no external sha256sum dependency).
pub fn file_sha256(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    Some(hex_lower(&crate::dsh_login::sha256(&data)))
}

/// SHA-256 of the running binary, if readable.
pub fn current_sha256() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    file_sha256(&exe)
}

fn our_sha256() -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate the running binary: {e}"))?;
    file_sha256(&exe).ok_or_else(|| format!("cannot read running binary {}", exe.display()))
}

fn resolve_stable_label(b: &str) -> String {
    let Ok(effective) = final_url(&format!("{b}/releases/latest")) else {
        return "stable".into();
    };
    let tag = effective
        .rsplit("/tag/")
        .next()
        .filter(|_| effective.contains("/tag/"))
        .unwrap_or("");
    if let Some(v) = tag_version(tag) {
        format!("v{v}")
    } else if !tag.is_empty() {
        tag.to_string()
    } else {
        "stable".into()
    }
}

/// Check the selected channel by SHA-256. `Ok(None)` means the running binary
/// already matches the published SHA-256 digest.
pub fn check(channel: Channel) -> Result<Option<UpdateInfo>, String> {
    let asset = asset_name().ok_or_else(|| {
        format!(
            "self-update is not available for {} (only linux aarch64/x86_64 builds are published)",
            std::env::consts::ARCH
        )
    })?;
    let b = base();
    let (sums_url, asset_url) = match channel {
        Channel::Stable => (
            format!("{b}/releases/latest/download/SHA256SUMS"),
            format!("{b}/releases/latest/download/{asset}"),
        ),
        Channel::Nightly => (
            format!("{b}/releases/download/nightly/SHA256SUMS"),
            format!("{b}/releases/download/nightly/{asset}"),
        ),
    };
    let sums = curl(&[&sums_url])?;
    let sha256 = parse_sha256sums(&sums)
        .get(asset)
        .cloned()
        .ok_or_else(|| format!("{} SHA256SUMS has no {asset}", channel.as_str()))?;
    let local_sha256 = our_sha256()?;
    if !needs_update(&local_sha256, &sha256) {
        return Ok(None);
    }
    let version = match channel {
        Channel::Stable => resolve_stable_label(&b),
        Channel::Nightly => "nightly".into(),
    };
    Ok(Some(UpdateInfo {
        channel,
        version,
        asset: asset.to_string(),
        url: asset_url,
        sha256,
        local_sha256,
    }))
}

/// Download, verify and atomically replace the running binary.
pub fn install(info: &UpdateInfo, mut status: impl FnMut(&str)) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate the running binary: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    let tmp = dir.join(format!(".dst.update.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    status("downloading");
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "300", "-A", USER_AGENT, "-o"])
        .arg(&tmp)
        .arg(&info.url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("download failed: {}", err.trim()));
    }

    status("verifying");
    let got = file_sha256(&tmp).ok_or_else(|| "cannot hash the downloaded binary".to_string())?;
    if !got.eq_ignore_ascii_case(&info.sha256) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "checksum mismatch: expected {}, got {got}",
            info.sha256
        ));
    }

    status("installing");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    // Rename over the running binary in the binary's own directory (same
    // filesystem, atomic); a running Linux executable may be replaced this way.
    std::fs::rename(&tmp, &exe).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!(
            "cannot replace {}: {e} — run dst as root or update with tools/deploy/deploy.sh",
            exe.display()
        )
    })?;
    Ok(())
}

/// Replace this process with the freshly installed binary (same argv).
pub fn restart_now() -> ! {
    crate::stty_tty(&["sane", "min", "1", "time", "0"]);
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("dst"));
    let err = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("dst: updated, but could not restart {}: {err}", exe.display());
    std::process::exit(1);
}

/// Should self-update run in this process? Debug builds (developer checkouts)
/// do not know which release they correspond to; `DST_UPDATE_FORCE=1` enables
/// the flow for tests.
pub fn enabled() -> bool {
    if std::env::var("DST_NO_UPDATE_CHECK").map(|v| v == "1").unwrap_or(false) {
        return false;
    }
    // A temporary trial binary must not offer to replace itself.
    if std::env::var("DST_PET_TRIAL").map(|v| v == "1").unwrap_or(false) {
        return false;
    }
    !cfg!(debug_assertions)
        || std::env::var("DST_UPDATE_FORCE")
            .map(|v| v == "1")
            .unwrap_or(false)
}

static PENDING: Mutex<Option<Result<Option<UpdateInfo>, String>>> = Mutex::new(None);

/// Background startup check: one thread, result collected by the TUI loop.
pub fn spawn_check(channel: Channel) {
    if !enabled() {
        return;
    }
    if let Ok(mut slot) = PENDING.lock() {
        if slot.is_some() {
            return; // a check is already in flight / finished
        }
        *slot = None;
    }
    std::thread::spawn(move || {
        let result = check(channel);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(result);
        }
    });
}

/// Take a finished background result, if any (`None` = nothing to report yet).
pub fn take_check() -> Option<Result<Option<UpdateInfo>, String>> {
    PENDING.lock().ok().and_then(|mut slot| slot.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_parse_and_default_to_nightly() {
        assert_eq!(Channel::parse("nightly"), Some(Channel::Nightly));
        assert_eq!(Channel::parse(" Stable "), Some(Channel::Stable));
        assert_eq!(Channel::parse("latest"), None);
        assert_eq!(Channel::Nightly.as_str(), "nightly");
    }

    #[test]
    fn sha256_comparison_ignores_case_and_detects_diff() {
        let a = "b10fcf7d281a5e3611a70f47e6d8a2d372214190ca29aefa47eb0dcd982ba90c";
        let a_upper = "B10FCF7D281A5E3611A70F47E6D8A2D372214190CA29AEFA47EB0DCD982BA90C";
        let b = "e8798c0b26e01b8be5b56409f20a27096fb39d5b9a6be614cc81234f6d962d94";
        assert!(!needs_update(a, a));
        assert!(!needs_update(a, a_upper));
        assert!(needs_update(a, b));
        assert_eq!(short_sha(a), "b10fcf7d");
    }

    #[test]
    fn file_sha256_matches_standard_vector() {
        let p = std::env::temp_dir().join(format!("dst-sha256-test-{}.bin", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        let got = file_sha256(&p);
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            got.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    #[test]
    fn release_tags_are_normalized() {
        assert_eq!(tag_version("v0.2.1"), Some("0.2.1".into()));
        assert_eq!(tag_version("0.2"), Some("0.2".into()));
        assert_eq!(tag_version("nightly"), None);
        assert_eq!(tag_version("v"), None);
        assert_eq!(tag_version("v1.2.3-rc1"), None);
    }

    #[test]
    fn sha256sums_parse_the_github_layout() {
        let sums = "b10fcf7d281a5e3611a70f47e6d8a2d372214190ca29aefa47eb0dcd982ba90c  dst-linux-aarch64\n\
                    e8798c0b26e01b8be5b56409f20a27096fb39d5b9a6be614cc81234f6d962d94 *dst-linux-x86_64\n\
                    garbage line\n";
        let map = parse_sha256sums(sums);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("dst-linux-aarch64").map(|s| s.as_str()),
            Some("b10fcf7d281a5e3611a70f47e6d8a2d372214190ca29aefa47eb0dcd982ba90c")
        );
        assert!(map.contains_key("dst-linux-x86_64"));
    }

    #[test]
    fn asset_names_cover_the_published_platforms() {
        if std::env::consts::ARCH == "aarch64" {
            assert_eq!(asset_name(), Some("dst-linux-aarch64"));
        } else if std::env::consts::ARCH == "x86_64" {
            assert_eq!(asset_name(), Some("dst-linux-x86_64"));
        }
    }

    #[test]
    fn stable_channel_compares_sha256_not_version() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let Some(asset) = asset_name() else { return };
        let Some(local_sha) = current_sha256() else { return };
        let diff_sha = "0".repeat(64);
        assert_ne!(local_sha, diff_sha);

        // Serve a local HTTP mock for /releases/latest/download/SHA256SUMS and /releases/latest.
        let spawn_server = |sha: String, tag: String| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let asset_s = asset.to_string();
            std::thread::spawn(move || {
                for _ in 0..3 {
                    let Ok((mut stream, _)) = listener.accept() else { break };
                    let mut buf = [0u8; 1024];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("");
                    if path.ends_with("/SHA256SUMS") {
                        let body = format!("{sha}  {asset_s}\n");
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if path == "/releases/latest" {
                        let loc = format!("http://127.0.0.1:{port}/releases/tag/{tag}");
                        let resp = format!(
                            "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else {
                        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
            });
            format!("http://127.0.0.1:{port}")
        };

        // Case 1: Same Cargo version tag (v0.1.0), DIFFERENT SHA-256 -> MUST offer update!
        let base_diff = spawn_server(diff_sha.clone(), format!("v{}", env!("CARGO_PKG_VERSION")));
        std::env::set_var("DST_UPDATE_BASE", &base_diff);
        let info = check(Channel::Stable)
            .expect("stable check succeeds")
            .expect("different sha256 must offer update even when version tag matches");
        assert_eq!(info.sha256, diff_sha);
        assert_eq!(info.local_sha256, local_sha);
        assert_eq!(info.version, format!("v{}", env!("CARGO_PKG_VERSION")));

        // Case 2: Matching SHA-256 -> up to date (None).
        let base_same = spawn_server(local_sha.clone(), "v9.9.9".into());
        std::env::set_var("DST_UPDATE_BASE", &base_same);
        let none = check(Channel::Stable).expect("stable check succeeds");
        assert!(none.is_none(), "matching sha256 must report up to date");
        std::env::remove_var("DST_UPDATE_BASE");
    }
}
