//! Whale-girl pixels and the character fallback.
//!
//! The built-in frames in `pet_sprites.rs` are original art drawn for this
//! 320×170 panel (GPL-3.0 with the rest of dst). They are a girl with hair,
//! a face and a whale tail, not the whale silhouette from the splash and not
//! the portraits shipped by `dsh-whale-musume`.
//!
//! A user who has their own sprites can drop a pack in `~/.dst/pet-pack/`
//! (see `docs/pet/PACK.md`). Those files are loaded at runtime and are never
//! fetched by dst.

include!("pet_sprites.rs");

pub const PANEL_W: usize = 320;
pub const PANEL_H: usize = 170;
pub const SPRITE_W: usize = 28;
pub const SPRITE_H: usize = 32;
/// Transparent key for the offscreen overlay. Not a colour the pet uses.
pub const KEY: [u8; 3] = [255, 0, 255];

pub const DOCK_X: usize = 1 + 43 * 6; // 259
pub const DOCK_W: usize = 60;
pub const DOCK_H: usize = 10 * 14; // 140, rows 0..9
pub const ROOM_Y: usize = 1;
pub const ROOM_H: usize = 140;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    /// Idle: the girl stands in the sea across the transcript rows.
    Room,
    /// Working: the girl occupies the right-hand 60×140 dock.
    Dock,
}

#[derive(Clone, Debug)]
pub struct Paint {
    pub scene: Scene,
    pub pose: String,
    pub prop: String,
    pub mood: u8,
    pub satiety: u8,
    pub bond: u8,
    pub level: u16,
}

impl Paint {
    pub fn new(scene: Scene, pose: &str, prop: &str) -> Self {
        Self {
            scene,
            pose: pose.to_string(),
            prop: prop.to_string(),
            mood: 70,
            satiety: 80,
            bond: 10,
            level: 1,
        }
    }
}

pub fn palette(ch: char) -> Option<[u8; 3]> {
    Some(match ch {
        'o' => [26, 39, 68],
        'h' => [37, 72, 168],
        'b' => [61, 126, 255],
        's' => [255, 216, 198],
        'w' => [255, 255, 255],
        'p' => [255, 143, 184],
        'k' => [22, 22, 32],
        'm' => [214, 78, 112],
        't' => [168, 208, 255],
        'y' => [255, 206, 72],
        'g' => [57, 211, 83],
        'n' => [255, 244, 220],
        'd' => [18, 28, 58],
        'a' => [255, 176, 196],
        _ => return None,
    })
}

fn builtin(pose: &str) -> &'static [&'static str] {
    match pose {
        "blink" => BLINK,
        "think" => THINK,
        "talk" => TALK,
        "happy" => HAPPY,
        "sad" => SAD,
        "eat" => EAT,
        "blush" => BLUSH,
        "sleep" => SLEEP,
        _ => IDLE,
    }
}

/// Character-mode portrait. Every glyph is single-width (ASCII or U+2500–259F)
/// so it can sit in an fbterm cell. Nine rows, like the splash.
pub fn text_rows(pose: &str) -> &'static [&'static str] {
    match pose {
        "sleep" | "sad" => TEXT_SLEEP,
        "happy" | "eat" | "blush" | "talk" => TEXT_HAPPY,
        _ => TEXT_IDLE,
    }
}

const TEXT_IDLE: &[&str] = &[
    "                    .                  ",
    "                 ▄▄███▄▄               ",
    "               ▄█████████▄             ",
    "              ██  ▀▄ ▄▀  ██            ",
    "              ██    ▀    ██            ",
    "               ▀██▄▄▄▄██▀              ",
    "                 ▀█▀▀█▀                ",
    "                ▀▀    ▀▀               ",
    "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁",
];

const TEXT_HAPPY: &[&str] = &[
    "                    .                  ",
    "                 ▄▄███▄▄               ",
    "               ▄█████████▄             ",
    "              ██  ▀▄ ▄▀  ██            ",
    "              ██   \\__/  ██            ",
    "               ▀██▄▄▄▄██▀              ",
    "                 ▀█▀▀█▀                ",
    "                ▀▀    ▀▀               ",
    "▂▃▄▄▃▂▂▁▁▂▄▅▄▂▁▁▂▃▄▃▂▁▁▂▃▂▁▁▂▃▂▁▁▁▂▃▂▁▁",
];

const TEXT_SLEEP: &[&str] = &[
    "                    z                  ",
    "                 ▄▄███▄▄               ",
    "               ▄█████████▄             ",
    "              ██  ▀▀ ▀▀  ██            ",
    "              ██         ██            ",
    "               ▀██▄▄▄▄██▀              ",
    "                 ▀█▀▀█▀                ",
    "                ▀▀    ▀▀               ",
    "▁▁▂▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▂▁▁▁▁▁",
];

/// Three-cell ASCII face for the status row when there is no pixel dock.
pub fn badge(pose: &str) -> &'static str {
    match pose {
        "sleep" => "zZz",
        "think" => "?_?",
        "talk" => "o_o",
        "happy" => "^_^",
        "sad" => "x_x",
        "eat" => "nom",
        "blush" => ">_<",
        "tool" => ">_]",
        _ => "o_o",
    }
}

pub fn pose_sprite(pose: &str, tick: u64) -> &'static str {
    match pose {
        "idle" => {
            if tick % 16 == 0 { "blink" } else { "idle" }
        }
        "think" => {
            if tick % 10 == 0 { "blink" } else { "think" }
        }
        "talk" => {
            if tick % 2 == 0 { "talk" } else { "idle" }
        }
        "sleep" => "sleep",
        "happy" => "happy",
        "sad" => "sad",
        "eat" | "compact" => "eat",
        "blush" => "blush",
        "tool" => "idle",
        _ => "idle",
    }
}

/// Full 320×170 RGB888 overlay. `KEY` marks pixels the terminal still owns.
pub fn render(paint: &Paint, pack: Option<&Pack>) -> Vec<u8> {
    let mut buf = vec![0u8; PANEL_W * PANEL_H * 3];
    for px in buf.chunks_exact_mut(3) {
        px.copy_from_slice(&KEY);
    }
    match paint.scene {
        Scene::Room => paint_room(&mut buf, paint, pack),
        Scene::Dock => paint_dock(&mut buf, paint, pack),
    }
    buf
}

fn paint_room(buf: &mut [u8], paint: &Paint, pack: Option<&Pack>) {
    fill(buf, 0, ROOM_Y, PANEL_W, ROOM_H, [10, 18, 40]);
    for (x, y) in [(18, 10), (70, 22), (140, 8), (210, 18), (280, 12), (250, 30)] {
        put(buf, x, ROOM_Y + y, [200, 206, 220]);
    }
    for x in (0..PANEL_W).step_by(2) {
        let y = ROOM_Y + ROOM_H - 16 + if (x / 8) % 3 == 0 { 2 } else { 0 };
        put(buf, x, y, [70, 120, 190]);
    }
    let sprite = paint.pose.as_str();
    // 3× the 28×32 girl is 84×96, which still leaves the sea line visible.
    blit_pose(buf, pack, sprite, (PANEL_W - SPRITE_W * 3) / 2, ROOM_Y + 16, 3);
    if !paint.prop.is_empty() {
        blit_prop(buf, &paint.prop, PANEL_W / 2 + 36, ROOM_Y + 70, 2);
    }
}

fn paint_dock(buf: &mut [u8], paint: &Paint, pack: Option<&Pack>) {
    fill(buf, DOCK_X, 1, DOCK_W, DOCK_H, [14, 16, 20]);
    // 2× is 56×64, which fits the 60px column and leaves room for the bars.
    let ox = DOCK_X + (DOCK_W - SPRITE_W * 2) / 2;
    blit_pose(buf, pack, &paint.pose, ox, 4, 2);
    if !paint.prop.is_empty() {
        // Over the right side of the dress. A 1× prop at the feet disappeared
        // into the 60px column.
        blit_prop(buf, &paint.prop, DOCK_X + 40, 28, 2);
    }
    let bars = [(paint.mood, [255, 110, 160]), (paint.satiety, [255, 176, 64]), (paint.bond, [90, 190, 255])];
    for (i, (value, color)) in bars.iter().enumerate() {
        let y = 78 + i * 12;
        fill(buf, DOCK_X + 4, y, 52, 6, [40, 44, 54]);
        let w = (*value as usize).min(100) * 50 / 100;
        if w > 0 {
            fill(buf, DOCK_X + 5, y + 1, w, 4, *color);
        }
    }
    draw_text(buf, DOCK_X + 4, 116, &format!("L{}", paint.level.min(99)));
}

fn blit_pose(buf: &mut [u8], pack: Option<&Pack>, pose: &str, x: usize, y: usize, scale: usize) {
    if let Some(pack) = pack {
        if let Some(img) = pack.frame(pose).or_else(|| pack.frame("idle")) {
            blit_image(buf, img, x, y, SPRITE_W * scale, SPRITE_H * scale);
            return;
        }
    }
    blit_rows(buf, builtin(pose), x, y, scale);
}

fn blit_prop(buf: &mut [u8], name: &str, x: usize, y: usize, scale: usize) {
    let rows = prop_rows(name);
    if rows.is_empty() {
        return;
    }
    blit_rows(buf, rows, x, y, scale);
}

fn blit_rows(buf: &mut [u8], rows: &[&str], ox: usize, oy: usize, scale: usize) {
    for (ry, row) in rows.iter().enumerate() {
        for (rx, ch) in row.chars().enumerate() {
            let Some(color) = palette(ch) else { continue };
            for dy in 0..scale {
                for dx in 0..scale {
                    put(buf, ox + rx * scale + dx, oy + ry * scale + dy, color);
                }
            }
        }
    }
}

fn blit_image(buf: &mut [u8], img: &Image, ox: usize, oy: usize, tw: usize, th: usize) {
    if img.w == 0 || img.h == 0 || tw == 0 || th == 0 {
        return;
    }
    for y in 0..th {
        for x in 0..tw {
            let sx = x * img.w / tw;
            let sy = y * img.h / th;
            let i = (sy * img.w + sx) * 3;
            let px = [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]];
            if px == KEY {
                continue;
            }
            put(buf, ox + x, oy + y, px);
        }
    }
}

fn fill(buf: &mut [u8], x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
    for yy in y..(y + h).min(PANEL_H) {
        for xx in x..(x + w).min(PANEL_W) {
            put(buf, xx, yy, color);
        }
    }
}

fn put(buf: &mut [u8], x: usize, y: usize, color: [u8; 3]) {
    if x >= PANEL_W || y >= PANEL_H {
        return;
    }
    let i = (y * PANEL_W + x) * 3;
    buf[i..i + 3].copy_from_slice(&color);
}

/// 3×5 ASCII digits and `L`, enough for "L12" under the bars.
fn draw_text(buf: &mut [u8], mut x: usize, y: usize, text: &str) {
    for ch in text.chars() {
        if let Some(rows) = glyph(ch) {
            for (ry, row) in rows.iter().enumerate() {
                for (rx, bit) in row.chars().enumerate() {
                    if bit == '#' {
                        put(buf, x + rx, y + ry, [210, 214, 224]);
                    }
                }
            }
        }
        x += 4;
    }
}

fn glyph(ch: char) -> Option<[&'static str; 5]> {
    Some(match ch {
        '0' => ["###", "#.#", "#.#", "#.#", "###"],
        '1' => ["##.", ".#.", ".#.", ".#.", "###"],
        '2' => ["###", "..#", "###", "#..", "###"],
        '3' => ["###", "..#", "###", "..#", "###"],
        '4' => ["#.#", "#.#", "###", "..#", "..#"],
        '5' => ["###", "#..", "###", "..#", "###"],
        '6' => ["###", "#..", "###", "#.#", "###"],
        '7' => ["###", "..#", ".#.", ".#.", ".#."],
        '8' => ["###", "#.#", "###", "#.#", "###"],
        '9' => ["###", "#.#", "###", "..#", "###"],
        'L' => ["#..", "#..", "#..", "#..", "###"],
        _ => return None,
    })
}

pub fn ppm(rgb: &[u8]) -> Vec<u8> {
    let mut out = format!("P6\n{PANEL_W} {PANEL_H}\n255\n").into_bytes();
    out.extend_from_slice(rgb);
    out
}

/// User sprite pack. Each frame is a PPM (P6 or P3); magenta is transparent.
#[derive(Clone, Debug)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Pack {
    frames: Vec<(String, Image)>,
}

impl Pack {
    pub fn frame(&self, name: &str) -> Option<&Image> {
        self.frames.iter().find(|(n, _)| n == name).map(|(_, i)| i)
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Load `dir/idle.ppm` and any other `<pose>.ppm`. A missing directory is not
/// an error: the built-in girl is used. A directory that exists but holds no
/// readable frame is an error so the user learns the pack was not picked up.
pub fn load_pack(dir: &std::path::Path) -> Result<Option<Pack>, String> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut pack = Pack::default();
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for ent in rd.flatten() {
        let path = ent.path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if path.extension().and_then(|s| s.to_str()) != Some("ppm") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let img = decode_ppm(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        pack.frames.push((name.to_string(), img));
    }
    if pack.is_empty() {
        return Err(format!("{} has no .ppm frames", dir.display()));
    }
    if pack.frame("idle").is_none() {
        return Err(format!("{} needs idle.ppm", dir.display()));
    }
    Ok(Some(pack))
}

pub fn decode_ppm(bytes: &[u8]) -> Result<Image, String> {
    if bytes.starts_with(b"P6") {
        decode_p6(bytes)
    } else if bytes.starts_with(b"P3") {
        decode_p3(std::str::from_utf8(bytes).map_err(|_| "P3 is not utf-8")?)
    } else {
        Err("need a P3 or P6 PPM".into())
    }
}

fn decode_p6(bytes: &[u8]) -> Result<Image, String> {
    let mut i = 0;
    let mut header = Vec::new();
    while header.len() < 4 && i < bytes.len() {
        if bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
            continue;
        }
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() { i += 1; }
        header.push(std::str::from_utf8(&bytes[start..i]).unwrap_or("").to_string());
    }
    if header.len() < 4 || header[0] != "P6" || header[3] != "255" {
        return Err("bad P6 header".into());
    }
    let w: usize = header[1].parse().map_err(|_| "bad width")?;
    let h: usize = header[2].parse().map_err(|_| "bad height")?;
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return Err("frame size out of range (1..=1024)".into());
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
    let need = w * h * 3;
    if bytes.len() < i + need {
        return Err("P6 truncated".into());
    }
    Ok(Image { w, h, rgb: bytes[i..i + need].to_vec() })
}

fn decode_p3(text: &str) -> Result<Image, String> {
    let mut nums = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        for tok in line.split_whitespace() {
            nums.push(tok.to_string());
        }
    }
    if nums.first().map(String::as_str) != Some("P3") {
        return Err("bad P3".into());
    }
    if nums.len() < 4 {
        return Err("bad P3 header".into());
    }
    let w: usize = nums[1].parse().map_err(|_| "bad width")?;
    let h: usize = nums[2].parse().map_err(|_| "bad height")?;
    if nums[3] != "255" || w == 0 || h == 0 || w > 1024 || h > 1024 {
        return Err("bad P3 size".into());
    }
    let need = w * h * 3;
    if nums.len() < 4 + need {
        return Err("P3 truncated".into());
    }
    let mut rgb = Vec::with_capacity(need);
    for tok in &nums[4..4 + need] {
        let v: u16 = tok.parse().map_err(|_| "bad sample")?;
        if v > 255 { return Err("sample > 255".into()); }
        rgb.push(v as u8);
    }
    Ok(Image { w, h, rgb })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_frames_are_complete_and_single_byte() {
        for (name, rows) in [
            ("idle", IDLE), ("blink", BLINK), ("think", THINK), ("talk", TALK),
            ("happy", HAPPY), ("sad", SAD), ("eat", EAT), ("blush", BLUSH), ("sleep", SLEEP),
        ] {
            assert_eq!(rows.len(), SPRITE_H, "{name}");
            for row in rows {
                assert_eq!(row.len(), SPRITE_W, "{name} {row}");
                assert!(row.chars().all(|c| c == '.' || palette(c).is_some()));
            }
        }
    }

    #[test]
    fn the_girl_is_not_a_whale_silhouette() {
        // Hair pixels sit above the face, and a tail notch splits the flukes.
        assert!(IDLE[0].contains('d') || IDLE[1].contains('d'), "hair");
        assert!(IDLE[9].contains('k') || IDLE[10].contains('k'), "eyes");
        assert!(IDLE[30].contains('.'), "tail notch");
        let hair = IDLE.iter().take(6).filter(|r| r.contains('h') || r.contains('d')).count();
        assert!(hair >= 4, "a head of hair, not a whale back");
    }

    #[test]
    fn text_portrait_is_single_width() {
        for pose in ["idle", "sleep", "happy"] {
            let rows = text_rows(pose);
            assert_eq!(rows.len(), 9, "{pose}");
            for row in rows {
                assert!(row.chars().count() <= 53, "{row}");
                assert!(
                    row.chars().all(|c| c.is_ascii() || ('\u{2500}'..='\u{259f}').contains(&c)),
                    "{pose} wide glyph: {row}"
                );
            }
        }
    }

    #[test]
    fn dock_leaves_the_transcript_transparent_and_draws_the_girl() {
        let buf = render(&Paint::new(Scene::Dock, "idle".into(), "shell".into()), None);
        assert_eq!(&buf[0..3], &KEY);
        // A pixel inside the 2× sprite (dock origin + a hair pixel).
        let x = DOCK_X + 8;
        let y = 10;
        let i = (y * PANEL_W + x) * 3;
        assert_ne!(&buf[i..i + 3], &KEY);
        // Below the dock the terminal owns the row.
        let i = (150 * PANEL_W + DOCK_X) * 3;
        assert_eq!(&buf[i..i + 3], &KEY);
    }

    #[test]
    fn room_covers_the_sea_and_not_the_input_row() {
        let buf = render(&Paint::new(Scene::Room, "sleep".into(), "".into()), None);
        let sea = (20 * PANEL_W + 4) * 3;
        assert_eq!(&buf[sea..sea + 3], &[10, 18, 40]);
        let input = (160 * PANEL_W + 10) * 3;
        assert_eq!(&buf[input..input + 3], &KEY);
    }

    #[test]
    fn ppm_roundtrip_and_pack() {
        let buf = render(&Paint::new(Scene::Dock, "happy".into(), "".into()), None);
        let img = decode_ppm(&ppm(&buf)).unwrap();
        assert_eq!((img.w, img.h), (PANEL_W, PANEL_H));
        let dir = std::env::temp_dir().join(format!("dst-pet-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("idle.ppm"), ppm(&buf)).unwrap();
        let pack = load_pack(&dir).unwrap().unwrap();
        assert!(pack.frame("idle").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
