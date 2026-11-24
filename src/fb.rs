//! The Cardputer Zero panel, written only after it has been identified.
//!
//! `login.rs` opens `/dev/fb_panel` for the QR code. The hardware note names
//! the same panel `/dev/fb_lcd` and warns that `/dev/fb0` is often HDMI.
//! This module checks sysfs before writing: 16 bits per pixel and a
//! 320×170 virtual size, and (unless the path was set explicitly) a device
//! name that looks like the ST7789 panel. An explicit `pet.fb` or `DST_FB`
//! still has to pass the geometry check, so a wrong path is not painted.
//!
//! Nothing here runs on the simulator. `DST_PET_FB_OUT` is an offscreen PPM
//! written by the pet, not a framebuffer.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 170;

#[derive(Clone, Debug)]
pub struct Info {
    #[allow(dead_code)]
    pub path: PathBuf,
    #[allow(dead_code)]
    pub name: String,
    pub stride: usize,
    /// True when the user named this node (`pet.fb` or `DST_FB`).
    #[allow(dead_code)]
    pub explicit: bool,
}

pub struct Panel {
    file: File,
    pub info: Info,
    /// Pixels that were on the panel before this process drew anything.
    /// Restored on conceal, drop, and a caught signal. Never a black fill:
    /// that would replace whatever fbterm had drawn.
    backup: Vec<u8>,
}

struct SavedPanel {
    path: PathBuf,
    stride: usize,
    bytes: Vec<u8>,
}

static SAVED_PANEL: Mutex<Option<SavedPanel>> = Mutex::new(None);
static SHUTDOWN_SIGNAL: AtomicI32 = AtomicI32::new(0);

/// `override_path` comes from `pet.fb` or `DST_FB`. Empty means probe.
pub fn open(override_path: &str) -> Result<Panel, String> {
    let explicit = !override_path.trim().is_empty();
    let candidates: Vec<PathBuf> = if explicit {
        vec![PathBuf::from(override_path.trim())]
    } else {
        probe_paths()
    };
    let mut why = Vec::new();
    for path in candidates {
        match inspect(&path, explicit) {
            Ok(info) => match File::options().read(true).write(true).open(&path) {
                Ok(mut file) => match snapshot(&mut file, info.stride) {
                    Ok(bytes) => {
                        if let Ok(mut slot) = SAVED_PANEL.lock() {
                            *slot = Some(SavedPanel {
                                path: info.path.clone(),
                                stride: info.stride,
                                bytes: bytes.clone(),
                            });
                        }
                        return Ok(Panel { file, info, backup: bytes });
                    }
                    Err(e) => why.push(format!(
                        "{}: refusing to draw; could not read the current pixels ({e})",
                        path.display()
                    )),
                },
                Err(e) => why.push(format!("{}: {e}", path.display())),
            },
            Err(e) => why.push(e),
        }
    }
    Err(if why.is_empty() { "no panel framebuffer".into() } else { why.join("; ") })
}

fn probe_paths() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/dev/fb_lcd"), PathBuf::from("/dev/fb_panel")];
    if let Ok(rd) = std::fs::read_dir("/sys/class/graphics") {
        let mut extra = Vec::new();
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if !name.starts_with("fb") || name == "fbcon" { continue; }
            let dev = PathBuf::from("/dev").join(&name);
            if !out.contains(&dev) {
                extra.push(dev);
            }
        }
        extra.sort();
        out.extend(extra);
    }
    out
}

fn inspect(path: &Path, explicit: bool) -> Result<Info, String> {
    let node = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let sys = sysfs_dir(node).ok_or_else(|| format!("{}: no sysfs node", path.display()))?;
    let bpp = read_num(&sys.join("bits_per_pixel")).unwrap_or(0);
    let (w, h) = virtual_size(&sys.join("virtual_size")).unwrap_or((0, 0));
    let stride = read_num(&sys.join("stride")).or_else(|| read_num(&sys.join("line_length"))).unwrap_or(0);
    let dev_name = std::fs::read_to_string(sys.join("name")).unwrap_or_default().trim().to_string();
    // Measured panel: /dev/fb_lcd -> fb0, name like panel-mipi-dbi, 16bpp,
    // virtual 320x170, stride 640. A taller virtual height is a double buffer
    // and is refused. The login is passed in by the operator; it is not a
    // default baked into this file.
    if !geometry_ok(bpp, w, h, stride) {
        return Err(format!(
            "{}: bpp {bpp} size {w}x{h} stride {stride} name {dev_name} (need 16bpp 320x170)",
            path.display()
        ));
    }
    if !explicit && !name_is_panel(&dev_name, node) {
        return Err(format!("{}: name {dev_name:?} is not the ST7789 panel", path.display()));
    }
    Ok(Info { path: path.to_path_buf(), name: dev_name, stride, explicit })
}

fn geometry_ok(bpp: usize, w: usize, h: usize, stride: usize) -> bool {
    bpp == 16 && w == WIDTH && h == HEIGHT && stride >= WIDTH * 2
}

fn name_is_panel(dev_name: &str, node: &str) -> bool {
    let n = dev_name.to_ascii_lowercase();
    node == "fb_lcd" || node == "fb_panel" || n.contains("st7789") || n.contains("mipi") || n.contains("panel")
}

fn sysfs_dir(node: &str) -> Option<PathBuf> {
    let direct = PathBuf::from("/sys/class/graphics").join(node);
    if direct.join("bits_per_pixel").is_file() {
        return Some(direct);
    }
    // /dev/fb_lcd is a udev alias. Match the graphics node whose `dev`
    // (major:minor) equals the device node.
    let meta = std::fs::metadata(Path::new("/dev").join(node)).ok()?;
    let rdev = std::os::unix::fs::MetadataExt::rdev(&meta);
    let major = rdev >> 8;
    let minor = rdev & 0xff;
    let want = format!("{major}:{minor}");
    let rd = std::fs::read_dir("/sys/class/graphics").ok()?;
    for ent in rd.flatten() {
        let dev = std::fs::read_to_string(ent.path().join("dev")).unwrap_or_default();
        if dev.trim() == want {
            return Some(ent.path());
        }
    }
    None
}

fn read_num(path: &Path) -> Option<usize> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn virtual_size(path: &Path) -> Option<(usize, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut it = text.trim().split(',');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

impl Panel {
    /// Write an RGB888 buffer. Magenta pixels are left untouched so the
    /// terminal keeps the cells it owns. RGB565 is little-endian, the order
    /// the ST7789 fbdev node on this image uses; the true-device checklist
    /// says to flip it if red and blue come out swapped.
    pub fn blit_rgb(&mut self, rgb: &[u8]) -> Result<(), String> {
        if rgb.len() != WIDTH * HEIGHT * 3 {
            return Err("overlay is not 320x170".into());
        }
        const KEY: [u8; 3] = [255, 0, 255];
        // Write contiguous non-transparent spans so the terminal cells we do
        // not own stay untouched. Magenta is the transparent key.
        for y in 0..HEIGHT {
            let mut x = 0;
            while x < WIDTH {
                let i = (y * WIDTH + x) * 3;
                if [rgb[i], rgb[i + 1], rgb[i + 2]] == KEY {
                    x += 1;
                    continue;
                }
                let start = x;
                let mut span = Vec::new();
                while x < WIDTH {
                    let j = (y * WIDTH + x) * 3;
                    let px = [rgb[j], rgb[j + 1], rgb[j + 2]];
                    if px == KEY { break; }
                    let value = rgb565(px);
                    span.push((value & 0xff) as u8);
                    span.push((value >> 8) as u8);
                    x += 1;
                }
                let at = (y * self.info.stride + start * 2) as u64;
                self.file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
                self.file.write_all(&span).map_err(|e| e.to_string())?;
            }
        }
        self.file.flush().ok();
        Ok(())
    }

    /// Put back the pixels captured when the panel was opened.
    pub fn restore(&mut self) -> Result<(), String> {
        write_rows(&mut self.file, self.info.stride, &self.backup)
    }

    /// Paint every visible pixel. The live exit path restores the snapshot
    /// instead, so a trial does not replace the previous screen with black.
    #[allow(dead_code)]
    pub fn fill(&mut self, rgb: [u8; 3]) -> Result<(), String> {
        let value = rgb565(rgb);
        let px = [(value & 0xff) as u8, (value >> 8) as u8];
        let row = px.repeat(WIDTH);
        for y in 0..HEIGHT {
            self.file.seek(SeekFrom::Start((y * self.info.stride) as u64)).map_err(|e| e.to_string())?;
            self.file.write_all(&row).map_err(|e| e.to_string())?;
        }
        self.file.flush().ok();
        Ok(())
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn snapshot(file: &mut File, stride: usize) -> Result<Vec<u8>, String> {
    let n = stride.checked_mul(HEIGHT).ok_or("framebuffer stride overflows")?;
    // A 320x170 panel at stride 640 is 108800 bytes. Refuse anything larger
    // than a 4K row so a wrong node cannot allocate a huge backup.
    if n == 0 || n > 4096 * HEIGHT {
        return Err(format!("framebuffer backup of {n} bytes is not the 320x170 panel"));
    }
    let mut bytes = vec![0u8; n];
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn write_rows(file: &mut File, stride: usize, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != stride * HEIGHT {
        return Err("framebuffer backup does not match the panel".into());
    }
    for y in 0..HEIGHT {
        let start = y * stride;
        file.seek(SeekFrom::Start(start as u64)).map_err(|e| e.to_string())?;
        file.write_all(&bytes[start..start + stride]).map_err(|e| e.to_string())?;
    }
    file.flush().ok();
    Ok(())
}

/// Write the snapshot taken at open back to the device. Safe to call when
/// no panel was opened. Used from the panic hook, which runs before `Drop`.
pub fn restore_saved_panel() {
    let saved = match SAVED_PANEL.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(saved) = saved.as_ref() else { return };
    if let Ok(mut file) = File::options().write(true).open(&saved.path) {
        let _ = write_rows(&mut file, saved.stride, &saved.bytes);
    }
}

const SIGINT: i32 = 2;
const SIGHUP: i32 = 1;
const SIGTERM: i32 = 15;

#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    restorer: usize,
}

unsafe extern "C" {
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    #[cfg(test)]
    fn raise(sig: i32) -> i32;
}

extern "C" fn on_shutdown_signal(sig: i32) {
    SHUTDOWN_SIGNAL.store(sig, Ordering::SeqCst);
}

/// Catch SIGTERM, SIGINT and SIGHUP so `timeout` can unwind through the
/// normal exit path. The handler only stores the signal number. `SA_RESTART`
/// is left off so a blocked read returns and the main loop can restore the
/// terminal and the framebuffer.
pub fn install_shutdown_signals() {
    unsafe {
        let act = SigAction {
            handler: on_shutdown_signal as usize,
            mask: [0; 16],
            flags: 0,
            restorer: 0,
        };
        for sig in [SIGINT, SIGHUP, SIGTERM] {
            let _ = sigaction(sig, &act, std::ptr::null_mut());
        }
    }
}

pub fn shutdown_requested() -> Option<i32> {
    let sig = SHUTDOWN_SIGNAL.load(Ordering::SeqCst);
    if sig == 0 { None } else { Some(sig) }
}

#[cfg(test)]
pub fn clear_shutdown_for_test() {
    SHUTDOWN_SIGNAL.store(0, Ordering::SeqCst);
}

pub fn rgb565(px: [u8; 3]) -> u16 {
    let r = (px[0] as u16) >> 3;
    let g = (px[1] as u16) >> 2;
    let b = (px[2] as u16) >> 3;
    (r << 11) | (g << 5) | b
}

/// True when `fbterm` is an ancestor, so an SSH or tmux dst does not paint
/// the handheld panel out from under the session that owns it.
pub fn fbterm_ancestor() -> bool {
    if std::env::var_os("DST_PET_FB_FORCE").is_some() {
        return true;
    }
    let mut pid = std::process::id();
    for _ in 0..6 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { break };
        let rest = match stat.rfind(')') { Some(i) => &stat[i + 1..], None => break };
        let mut fields = rest.split_whitespace();
        let _state = fields.next();
        let Some(ppid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else { break };
        if ppid <= 1 { break; }
        let comm = std::fs::read_to_string(format!("/proc/{ppid}/comm")).unwrap_or_default();
        if comm.trim() == "fbterm" { return true; }
        pid = ppid;
    }
    false
}

pub fn write_ppm(path: &Path, rgb: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(path, crate::pet_art::ppm(rgb)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_packs_channels() {
        assert_eq!(rgb565([255, 0, 0]), 0xF800);
        assert_eq!(rgb565([0, 255, 0]), 0x07E0);
        assert_eq!(rgb565([0, 0, 255]), 0x001F);
        assert_eq!(rgb565([255, 255, 255]), 0xFFFF);
    }

    #[test]
    fn a_non_panel_name_is_refused() {
        assert!(!name_is_panel("vc4drmfb", "fb0"));
        assert!(name_is_panel("st7789v", "fb1"));
        assert!(name_is_panel("drm", "fb_lcd"));
        assert!(name_is_panel("whatever", "fb_panel"));
        assert!(name_is_panel("panel-mipi-dbi", "fb0"));
        assert!(name_is_panel("panel-mipi-dbid", "fb0"));
        assert!(!name_is_panel("HDMI-A-1", "fb1"));
    }

    #[test]
    fn a_backup_round_trip_puts_the_original_pixels_back() {
        let path = std::env::temp_dir().join(format!(
            "dst-fb-backup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let stride = WIDTH * 2;
        let mut original = vec![0u8; stride * HEIGHT];
        for (i, b) in original.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        std::fs::write(&path, &original).unwrap();
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let backup = {
            let mut file = File::options().read(true).write(true).open(&path).unwrap();
            snapshot(&mut file, stride).unwrap()
        };
        assert_eq!(backup, original);
        let mut panel = Panel {
            file,
            info: Info {
                path: path.clone(),
                name: "test".into(),
                stride,
                explicit: true,
            },
            backup,
        };
        panel.fill([255, 0, 0]).unwrap();
        panel.restore().unwrap();
        drop(panel);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after, original);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sigterm_is_caught_instead_of_killing_the_process() {
        install_shutdown_signals();
        clear_shutdown_for_test();
        unsafe {
            raise(SIGTERM);
        }
        assert_eq!(shutdown_requested(), Some(SIGTERM));
        clear_shutdown_for_test();
    }

    #[test]
    fn the_measured_panel_is_accepted_and_a_double_buffer_is_not() {
        assert!(geometry_ok(16, 320, 170, 640));
        assert!(!geometry_ok(16, 320, 340, 640), "virtual height above 170 is a second buffer");
        assert!(!geometry_ok(32, 320, 170, 1280));
        assert!(!geometry_ok(16, 1920, 1080, 3840));
        assert!(!geometry_ok(16, 320, 170, 100));
    }
}
