---
name: hardware-info
description: Use when a task involves this Cardputer Zero's hardware - display, audio, keyboard/input, battery and power, WiFi/network, thermal, USB, I2C/SPI/GPIO, sensors - or before writing a device-specific app, so the code matches the real hardware.
origin: dst
version: 1
updated: 2026-11-18
---

# Cardputer Zero hardware reference

This package is the device's own hardware notebook. `dst` keeps it current: the section after the scan marker is rewritten from a real device scan by
`/update-hwinfo` (offered once during first-run setup), while the reference below is stable knowledge. Read it before writing anything that touches
hardware, and prefer the measured facts in "Current state" over assumptions.

This is a **Raspberry Pi CM0 Linux handheld**, *not* the ESP32-S3 Cardputer or Cardputer ADV (different SoC, 320x170 vs 240x135 screen, 46 vs 56
keys). Never apply ESP32/UIFlow/M5Burner instructions here, even though the 14-pin "Cap" expansion port is inherited from the ADV.

## Variants

Two hardware variants share one OS image; CM0, screen, keyboard, battery, speaker/mic, IR, radios, Grove and EXT port are identical.

| | Cardputer Zero (standard) | Cardputer Zero Lite |
| --- | --- | --- |
| 8 MP IMX219 camera | yes | **no** (Camera app disabled) |
| BMI270 IMU + BMM150 magnetometer | yes | **no** (Compass app disabled) |
| Bundled 32 GB microSD with OS | yes | **no** - supply a card (16 GB+) and flash it |
| MSRP / Kickstarter early-bird | $149 / $89 | $99 / $59 |

Transparent Black was a free colour choice for both models, not a variant; SKUs are C154 / C154-B / C155 / C155-B (suffix meaning not public,
unverified). The factory image is built for board revision **V0.5** (`CONFIG_CARDPUTERO_V0_5=y`, CI release notes); older V0.3 overlays exist with
different amp/IMU wiring - check which overlay `config.txt` loads before trusting pin/IRQ details. The OS manifest publishes one Trixie image for both
SKUs; a "lite" image profile exists in CI but is not in the public manifest (unverified whether any ships).

## Components

| Part | What it is | Notes for code |
| --- | --- | --- |
| SoM | Raspberry Pi CM0 Lite (no eMMC), RP3A0 SiP | M5 docs say "RP3A0 (BCM2837)", Raspberry Pi/CNX say BCM2710A1 - same RP3A0 family, docs disagree; don't rely on the label |
| CPU | 4x Cortex-A53 @ 1.0 GHz, ARMv8-A aarch64 | build arm64; dst budgets ~one slow core, and the device must never compile (OOM) |
| GPU/VPU | VideoCore IV (GLES 1.1/2.0), H.264 1080p30 enc/dec | KMS/DRM is active, but dst/fbterm are software-rendered; no GLES win |
| RAM | 512 MB LPDDR2 | ~446 MB visible on this unit (measured earlier; factory `gpu_mem=128` makes stock units show less - read MemTotal) |
| Storage | microSD (CM0 Lite has no eMMC) | ext4 root + FAT32 `/boot/firmware`; slow writes, app state in `$DST_APP_STATE` |
| Display | Sitronix ST7789v3, 320x170, SPI0 CS0 | RGB565, DC=GPIO25, CS=GPIO8, SPI <=60 MHz, TE=GPIO5 unused; DRM `panel-mipi-dbi` + firmware `cardputerzero,st7789v_lcd.bin` |
| Backlight | M5IOE1 PWM ch3 -> `pwm-backlight` | `/sys/class/backlight/backlight`, 0-100 brightness (default 75, 101-entry PWM table), forced on at probe |
| Keyboard | TI TCA8418RTWR, 5x10 matrix, 46 keys | I2C1 0x34, IRQ GPIO27; sym/shift/fn stickiness lives in the kernel driver |
| Audio codec | Everest ES8389 (driver matches `everest,es8390`) | I2C1 0x10, I2S GPIO18/19/20/21, MCLK 12.288 MHz fixed -> 48 kHz family |
| Speaker/amp | 1 W @ 8 ohm + AW8737A | amp enable GPIO24 (plain GPIO switch, fixed gain); the codec DAC is the only volume stage |
| Mic / jack | MEMS mic on ES8389 ADC; 3.5 mm TRRS out+mic | plug detect GPIO17 (auto speaker mute/reroute); CTIA/OMTP switch via M5IOE1 (docs; unverified in DT) |
| Fuel gauge | TI BQ27220YZFR | I2C1 0x55; `/sys/class/power_supply/bq27220-0`; 1750 mAh design capacity + NTC |
| RTC | Epson RX8130CE | I2C1 0x32, `/dev/rtc0`; no alarm IRQ in DT (alarm wake unverified) |
| IMU/mag | Bosch BMI270 0x68 + BMM150 0x10 (behind BMI270 aux bus) | Standard only; IIO devices named `bmi270`, `bmm150` |
| Camera | Sony IMX219 8 MP 3280x2464, 4-lane CSI | Standard only; `dtoverlay=imx219`, libcamera/Picamera2, `/dev/video0` + `/dev/media0` (numbering unverified) |
| IR | TX GPIO12 / RX GPIO13 | `gpio-ir` + `gpio-ir-tx` overlays -> `/dev/lirc*` (numbering unverified) |
| USB | GL852G-OHY60 hub: 1x USB-A 2.0 + 2x USB-C 2.0 | right USB-C2 = CM0 USB direct (slave/OTG/charge/rpiboot); left USB-C1 = hub-side |
| Ethernet | SR9900A USB2.0 -> 10/100 | through hub USB1, Host mode only; `eth0` |
| WiFi/BT | CM0 onboard 2.4 GHz 802.11b/g/n + BT 4.2 LE | `wlan0` / `hci0`, NetworkManager |
| Power | 3.7 V 1750 mAh LiPo with NTC (production; previews 1500 mAh) | charge via USB-C2 only, 5 V/2 A, ~2.5 W idle; gauge only, charger IC undocumented (unverified) |
| I/O expander | M5Stack M5IOE1 @ I2C1 0x4F | 14 GPIO, 4 PWM, ADC/temp via IIO; owns backlight, LCD reset, power rails, Grove/HAT switches, indicator LEDs |
| Expansion | HY2.0-4P Grove + EXT 2.54-14P | Grove = I2C1 (G2/G3) or UART (G14/G15) via G4, 5 V switched (<500 mA). EXT = SPI0, I2C1, UART, G26/G23 (GPIO or USB4 via HAT switch), 5VIN/5VOUT |
| Size / weight | 84.0 x 54.0 x 23.1 mm, 95 g | package 100 x 61 x 27.5 mm, 122 g; controls: side Boot, side USB-SW slider, top-right power switch; no separate reset button documented |

## Versions and factory software state

These are the pinned "known good" values for the shipping image; the scan section reports what this unit actually runs right now.

| Item | Version / identity |
| --- | --- |
| OS image | "CardputerZero OS (Trixie arm64)", tag `20260929-070146-5f4c44f`, released 2026-09-29, `init_format: cloudinit-rpi` |
| Userland | Debian 13 "trixie" arm64, Raspberry Pi OS based (review unit reported 13.6; read `/etc/debian_version`) |
| Kernel | `1:6.18.39-1+rpt1` (`linux-image-rpi-v8`), pinned by `/etc/apt/preferences.d/cardputerzero-kernel` and apt-held |
| Launcher | APPLaunch `0.6.105`, LVGL `9.5` (app template pins 9.5.0) |
| Drivers | `m5stack/m5stack-linux-dtoverlays` @ `7551854` (recorded in `/etc/cardputerzero-dtoverlays.commit`); out-of-tree modules in `/lib/modules/<kver>/extra/`: `st7789v_m5stack`, `pwm_bl_m5stack`, `panel-mipi-dbi-m`, `tca8418_keypad_m5stack`, `es8389_m5stack`, `bq27xxx_battery`(+`_i2c`), `m5ioe1`, `cardputerzero-audio`, `powerfail_suo`, `gpio-forwarder`; in-tree: `bmi270`, `bmm150` |
| Build/deploy | CMake >= 3.31 + SCons + aarch64 cross toolchain; apps are arm64 `.deb` installed under `/usr/share/APPLaunch` |
| Audio stack | PipeWire + WirePlumber per-user session, factory default sink volume 60% (0.216 linear); `pulseaudio-utils` |
| Network stack | NetworkManager (Wi-Fi), `bluez`, `avahi-daemon`, OpenSSH; factory cmdline sets `cfg80211.ieee80211_regdom=AE`; WLAN blocked until a country is set |
| Host identity | hostname `cardputerzero`; build defaults user `pi`/`raspberry` (M5 Imager/OOBE can change it); this dst unit is `hanxiao` (deploy target `hanxiao@192.168.43.49`) |
| `config.txt` | `gpu_mem=128`, `splash_screen=CardputerZero_splash_custom.bin`, `dtoverlay=vc4-kms-v3d,cma-64`, `max_framebuffers=2`, `dtparam=i2c_arm/i2s/spi=on`, `dtparam=audio=off`, `camera_auto_detect=0` + `dtoverlay=imx219`, `enable_uart=1`, `dtoverlay=dwc2`, V0.5 overlays `cardputerzero-v5-overlay`, `bq27220_v5`, `bmi270_bmm150_overlay`, `gpio-ir` (GPIO13), `gpio-ir-tx` (GPIO12) |
| `cmdline.txt` | `quiet fbcon=map:off cfg80211.ieee80211_regdom=AE` plus Raspberry Pi root=PARTUUID defaults; serial console deliberately removed |
| Services | `LaunchWizard.service` (root, first-boot OOBE + keyboard guide), `APPLaunch.service` (user unit, `After=pipewire-pulse.service`), `fb_load.service`, `ssh.service`, `avahi-daemon`, `rtkit-daemon`, `pipewire*`; `systemd-backlight@backlight:backlight` masked; ADB (`adbd`, `cardputer-adb-hotplug`) installed but off by default; journald persistent, 50 MB cap |
| Desktop / HDMI | default target `graphical.target`: labwc/lightdm desktop on HDMI (1080p30) when connected; internal panel driven separately. `fb-mirror.py` mirrors the LCD onto the HDMI desktop |
| Updates | apps via Settings > System > Software, or APT from the CardputerZero Debian repo (`m5stack/CardputerZeroRepository`; check `/etc/apt/sources.list.d/`); OS reinstall via M5 Imager (or rpiboot). After adding an APPLaunch `.desktop` entry restart `APPLaunch.service` |
| dst overlay | `/usr/local/bin/dst` + local `/etc/systemd/system/cpzterm.service` (fbterm + dst on tty1, 53x12 cells); the unit also needs the console keymap unit at boot - `tools/deploy/fix-cpzterm-order.sh` repairs an ordering cycle |
| Known limits | no suspend/hibernation (run or off); Lite has no camera/IMU; Cap modules are not hot-pluggable; 512 MB RAM (a release build OOM-kills); SD wear on frequent writes |

## Display and terminal

- The panel is a 320x170 RGB565 SPI LCD on a DRM panel driver (`vc4-kms-v3d` drives HDMI); udev creates **`/dev/fb_lcd`** (internal LCD) and
  **`/dev/dri/cardputer-zero-internal`** (DRM card, ID_PATH `platform-3f204000.spi-cs-0`).
- **Never hardcode `/dev/fb0`**: fb numbering depends on probe order and fb0 is often the HDMI framebuffer. Use `/dev/fb_lcd`, or scan `/proc/fb`
  and `/sys/class/graphics/fb*/name` for the st7789 entry - the most common wrong-framebuffer bug in third-party apps.
- dst sees the panel through fbterm on tty1: a 6x13 px cell gives **53 columns x 12 rows** (6x14 px pitch, 1 px margin -> 318x168 used of
  320x170). Everything dst draws must fit that grid; the console font has no emoji/wide glyphs.
- The factory image does **not** install fbterm: its panel UI is APPLaunch (LVGL on the framebuffer) plus the boot splash, and its CLI app is an
  LVGL terminal (ptmx-based). fbterm exists here because the dst deploy adds `cpzterm.service`. `fbcon=map:off` means no text console appears on
  the panel at boot.
- Brightness: `/sys/class/backlight/backlight/brightness` (0-100; also in the components table). The panel driver's `.../device/reset` attribute re-runs the init sequence; the physical LCD reset goes through the M5IOE1 expander (product pinmap; the V0.5 DT claims no reset GPIO).

## Keyboard and input

- One evdev keyboard from the TCA8418 matrix: `/dev/input/cardputer-zero-internal` (udev symlink; by-path `platform-3f804000.i2c-event`), input
  name `tca8418`, `EV_REP` enabled; 46 keys in a 5x10 matrix (4 positions unused). The `sym`/`shift`/`fn` indicator LEDs are driven by the M5IOE1,
  and the sticky modifier state machine is in the kernel driver: hold = active while held, one tap = one-shot, double tap = locked (LED fast
  blink).
- The fn layer emits standard Linux keycodes, so arrows work everywhere: fn+F/X/Z/C = Up/Down/Left/Right, fn+K/N = Home/End, fn+L/M =
  PageUp/PageDown, fn+Backspace = Delete, fn+B = Insert, fn+1..0/O/P = F1-F12, fn+U/I = brightness, fn+Q/W/E = media, fn+A mute, fn+S/D volume,
  fn+H help, fn+J screenshot; ctrl/alt are normal modifiers. ESC short = Escape, long = platform back/HOME; TAB short = Tab, long TAB is reserved
  for AI voice input (not implemented in factory apps; dst uses it for push-to-talk).
- The sym layer produces the printed blue symbols only when the **console keymap** is loaded:
  `/usr/share/keymaps/tca8418_keypad_m5stack_keymap.map` selected via `KMAP=...` in `/etc/default/keyboard` + `setupcon`. fbterm reads the VT and
  inherits it; APPLaunch reads evdev and maps itself. Symptom of a missing keymap: SYM+N prints `/` instead of the printed symbol.
- The keyboard shares I2C1 with the codec, RTC, gauge and expander: do not hammer the bus, never run `i2cdetect` on `/dev/i2c-1` casually, and
  bound every I2C read with a timeout.

## Audio

- ALSA card `ES8389Audio` (pretty name `ES8389-Audio`), PCM `ES8389 HiFi`; direct access `-D hw:CARD=ES8389Audio,DEV=0`. The fixed 12.288 MHz MCLK
  makes the 48 kHz family native (8/12/16/24/32/48/96 kHz); 44.1 kHz is not derivable without resampling (unverified hw-param detail). Measured on
  this unit: S32_LE stereo playback, s16 capture at 48 kHz.
- Headphone insert is detected on GPIO17 and handled in-kernel: the speaker path is muted and output reroutes to the jack. The speaker amp is a
  plain GPIO switch (fixed gain); the ES8389 DAC attenuator is the only volume stage.
- Factory default volume is 60% (WirePlumber's 40% was nearly inaudible). PipeWire/WirePlumber owns the card in the factory session; a direct
  `hw:` open bypasses it and races the session.
- **Measured quirk:** after the PCM opens, the path takes ~400 ms to unmute and drops the first frames - pad or discard ~0.5 s at the start of a
  recording.

## Power, battery and thermal

- Gauge: `/sys/class/power_supply/bq27220-0/` -> `capacity`, `status`, `voltage_now`, `current_now`, `temp` (deci-degC), `time_to_empty_now`,
  `cycle_count`, `health`; read-only. 1750 mAh design capacity. Charging: right USB-C2 only (on or off), red LED charging, green full, 5 V/2 A
  recommended; the BQ27220 only gauges, the charger IC is undocumented (unverified).
- Power switch (top-right): sliding left fires the power-fail IRQ (GPIO6, falling, 500 ms debounce); `powerfail_suo` remounts filesystems
  read-only, kills the backlight and asserts the power-disable line - a fast safe-off, not a clean systemd shutdown. Prefer `sudo poweroff` first,
  then switch off; M5's docs only say the switch "initiates the shutdown process".
- ~2.5 W idle, passive cooling. CPU temp: `/sys/class/thermal/thermal_zone0/temp` (milli-degC); a second zone may mirror battery temperature on
  some images (unverified - use `power_supply/temp` for the battery). No suspend-to-RAM; root is the microSD, so avoid write loops.

## Radios, USB and networking

- Wi-Fi 4 (2.4 GHz 802.11b/g/n) and BT 4.2 LE are on the CM0: `wlan0`, `hci0`; NetworkManager owns `wlan0` (dst never reconfigures it). The
  factory regdomain comes from `cfg80211.ieee80211_regdom=AE`; the country is the user's choice.
- Ethernet is a USB2.0-to-10/100 chip (SR9900A) behind the hub: `eth0`, Host mode only, sharing the hub with USB-A/USB-C1/HAT USB.
- USB-SW slider (left): **Slave** = CM0 USB to right USB-C2 (gadget/OTG, charge, rpiboot, ADB); hub, USB-A, left USB-C1, Ethernet and HAT USB
  disappear. **Host** = CM0 to the GL852G hub (USB-A, USB-C1, HAT-P0/P1 USB, Ethernet). ADB needs Slave mode + Settings > Developer > ADB.
- No serial console on the UART by default (`console=serial0` removed); GPIO14/GPIO15 are usable as `/dev/ttyAMA0` / `/dev/serial0` for Cap
  modules. Cap radios (LoRa SX1262, CC1101 sub-GHz/NFC) are external EXT-port modules, not hot-pluggable; ESP-NOW is an ESP32 feature and does not exist on this Linux device.

## Sensors, RTC and expansion

- IMU (Standard): BMI270 accel/gyro 0x68 and BMM150 magnetometer 0x10 behind the BMI270 aux bus; IIO `bmi270`/`bmm150`. Compass needs both (Lite:
  no).
- RTC: RX8130CE 0x32 -> `/dev/rtc0`; no alarm IRQ in DT, so RTC wake alarms are unverified/unsupported. IR: RX GPIO13 (`gpio-ir`, pull-up), TX
  GPIO12 (`gpio-ir-tx`) -> `/dev/lirc*` raw devices (numbering unverified).
- I2C1 ground truth (don't i2cdetect to find this): 0x10 ES8389, 0x32 RX8130, 0x34 TCA8418, 0x4F M5IOE1, 0x55 BQ27220, 0x68 BMI270 (Standard).
  BMM150 is not on the main bus (behind the BMI270).
- GPIO chips: `pinctrl-bcm2835` (board GPIO 0-27), `m5ioe1` (14 lines), and a virtual `gpio-spi-cs` forwarder; `gpiod`/`python3-libgpiod` exist.
  M5IOE1 also exposes 4 PWM channels (ch3 = backlight) and 4 ADC channels + TEMP/VREF via IIO (`ADC1_IO2`..`ADC4_IO7`); whether those are broken
  out on Grove/EXT is undocumented - treat as internal (unverified).
- Expansion rails are **off by default**: Grove 5 V / EXT 5 VOUT via Settings > ExtPort, writable as
  `/sys/class/leds/{grove_5v_out,ext_5v_out}/brightness`. Also here: `grove_fun` (0 = UART, 1 = I2C1 on the Grove port) and `ext_usb_gpio_fun`
  (HAT-P0/P1 as GPIO vs USB4). These are wiring switches - ask the user first.
- Camera (Standard): IMX219 on the 4-lane CSI via `dtoverlay=imx219`, libcamera/Picamera2 (`/dev/video0`, `/dev/media0`; numbering unverified).
  EXT 2.54-14P: SPI0 (G10/G11/G9/G7), I2C1 (G2/G3), UART (G14/G15), HAT-P0/P1 (G26/G23, GPIO by default or USB4 when switched), 5VIN and switched
  5VOUT. Power off before inserting/removing a Cap.

## How dst talks to the hardware

- **Panel**: `cpzterm.service` starts fbterm on tty1 and runs `dst` inside it; dst owns the tty. An app started by `app_run` inherits the same
  stdout/stdin and must stay inside 53x12 cells. Apps must never open `/dev/fb*`, `/dev/dri/*` or APPLaunch — the panel belongs to fbterm.
  dst's own whale-girl pet is the exception: it paints only after sysfs says the node is 16bpp and 320×170, and only into `/dev/fb_lcd`,
  `/dev/fb_panel`, or a path the user set with `pet.fb` / `DST_FB`. It does not open an unidentified `/dev/fbN`.
- **Keys**: a normal Linux console keyboard. The fn layer already emits arrow/paging keycodes, which the VT turns into `ESC[A`..`ESC[D`,
  `ESC[5~`/`ESC[6~`; long Esc is the platform back/HOME action. dst decodes sequences, apps read plain bytes.
- **Audio**: `arecord`/`aplay` with `-D hw:CARD=ES8389Audio,DEV=0` (dst uses these for ASR/TTS); remember the ~0.5 s unmute pad on capture.
- **Power/battery**: read `/sys/class/power_supply/bq27220-0/*` (read-only); temperature from `thermal_zone0`. **Network**: the OS owns
  `wlan0`/`eth0`; dst only uses `curl`/HTTPS and never reconfigures interfaces.

## How to probe (safe, read-only)

These are the commands the scanner runs; use them when you need a current answer. Every one is read-only.

```sh
uname -srm; head -2 /etc/os-release; cat /etc/debian_version
cat /proc/device-tree/model
grep -E 'model name|Hardware|processor' /proc/cpuinfo | head -2; nproc
grep -E 'MemTotal|MemAvailable|SwapTotal' /proc/meminfo; free -m
lsblk -o NAME,SIZE,TYPE,MOUNTPOINT; df -h /
cat /boot/firmware/cmdline.txt
grep -E '^(gpu_mem|dtoverlay|dtparam|splash)' /boot/firmware/config.txt
cat /proc/fb; for f in /sys/class/graphics/fb*; do echo "$f: $(cat $f/name) $(cat $f/virtual_size) $(cat $f/bits_per_pixel)"; done
ls -l /dev/fb_lcd /dev/input/cardputer-zero-internal 2>/dev/null
cat /sys/class/backlight/backlight/brightness /sys/class/backlight/backlight/max_brightness 2>/dev/null
cat /proc/asound/cards; head /proc/asound/pcm; aplay -l 2>/dev/null
grep -B1 -A3 -E 'Name="tca8418"' /proc/bus/input/devices | head -12
for p in /sys/class/power_supply/*; do echo "$p: $(cat $p/type) $(cat $p/status 2>/dev/null) $(cat $p/capacity 2>/dev/null) $(cat $p/temp 2>/dev/null)"; done
for z in /sys/class/thermal/thermal_zone*; do echo "$z: $(cat $z/type) $(cat $z/temp)"; done
for d in /sys/bus/iio/devices/iio:device*; do echo "$d: $(cat $d/name)"; done
for l in /sys/class/leds/*; do echo "$(basename $l)=$(cat $l/brightness 2>/dev/null)"; done
ls /dev/i2c-* /dev/spidev* /dev/gpiochip* /dev/rtc* /dev/lirc* /dev/video* /dev/media* /dev/ttyAMA* 2>/dev/null
for d in /sys/bus/i2c/devices/*; do [ -e "$d/name" ] && echo "$(basename $d)=$(cat $d/name)"; done
ip -br link; ip -br addr; iw dev 2>/dev/null | head -12
systemctl is-active cpzterm.service fb_load.service ssh 2>/dev/null; systemctl --user is-active APPLaunch.service 2>/dev/null
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null
```

Do **not** add `i2cdetect`, `i2cget` loops or PWM writes to a probe: bus 1 is shared with the keyboard, codec and expander, and a wedged transfer can
take the keyboard down with it (`i2cdetect -y 1` is not a harmless no-op here - reads can hang). Use the address map above instead.

## Rules for hardware code on this device

- Probe read-only first. Never change network, audio routing, backlight, GPIO direction, the 5 V rails (`grove_*`/`ext_*` LEDs), USB mode, charge
  settings, services or kernel overlays "to see what happens" - ask the user first.
- Everything you build runs inside dst's app contract: 53x12 cells, long-press Esc exits, no system configuration, no installs, no services, no
  root. App state only under `$DST_APP_STATE`.
- Keep polling cheap (>= 250 ms per read; the A53 is slow and the SD card slower) and stop when the app exits. Read thermals/charge on demand.
- Wrap reads that can hang (I2C, SPI, serial) in a timeout, never block on them, and never probe the panel/input/audio devices while dst owns
  them.
- Audio: expect 48 kHz, pad the first ~0.5 s of capture, and do not fight the PipeWire session (use `hw:` only for short, exclusive operations).
- Treat the power switch, Boot button and USB-SW slider as "ask the user" controls: power-off cuts the session, USB-SW cuts Ethernet/SSH, and Boot
  + power enters rpiboot (reflash mode).
- If a hardware fact matters and is not here, measure it with the probe commands and refresh this package with `/update-hwinfo`; mark anything you
  could not confirm as "(unverified)" instead of guessing.

## Sources

- M5Stack docs - specs/usage/image/ADB: https://docs.m5stack.com/en/CardputerZero , https://docs.m5stack.com/en/compute/cardputerzero/usage ,
  https://docs.m5stack.com/en/compute/cardputerzero/image , https://docs.m5stack.com/en/compute/cardputerzero/adb
- Developer docs (key map, app guidelines): https://cardputer.cc/#/documents (`docs/en/CP0_keys.md`, `CP0_dev.md`)
- OS manifest (image tag, kernel, launcher, driver commit): https://cardputer-zero-repo.oss-cn-shenzhen.aliyuncs.com/os-list.json
- Image build, services, boot config: https://github.com/CardputerZero/pi-gen
- Board drivers / DT overlays / udev+sysfs names: https://github.com/m5stack/m5stack-linux-dtoverlays
- Launcher + SDK: https://github.com/CardputerZero/launcher , https://github.com/m5stack/M5Stack_Linux_Libs
- App-development skill (fb device, evdev path, keymap, `.desktop` contract): https://github.com/CardputerZero/skill
- M5 Imager / org / Debian repo: https://github.com/CardputerZero/m5stack-imager , https://github.com/CardputerZero ,
  https://github.com/m5stack/CardputerZeroRepository
- openboardsguide hands-on (Debian 13.6, ports, runtime): https://www.openboardsguide.com/m5stack-cardputerzero-hands-on/
- CNX Software launch + CM0 specs (RP3A0/BCM2710A1, 4-lane CSI):
  https://www.cnx-software.com/2026/05/25/cardputerzero-a-raspberry-pi-cm0-pocket-computer-for-makers/ ,
  https://www.cnx-software.com/2025/09/23/raspberry-pi-cm0-castellated-module-features-raspberry-pi-rp3a0-system-in-package/
- Raspberry Pi CM0 page: https://www.raspberrypi.com/products/compute-module-zero/
- M5 launch blog (variant differences) + price page:
  https://shop.m5stack.com/blogs/news/m5stack-launches-cardputerzero-a-pocket-sized-linux-computer-for-makers-and-developers ,
  https://shop.m5stack.com/pages/m5-cardputerzero
- "Taming the CardputerZero" (fbcon, gpu_mem, keymap, bq27220 sysfs, thermal, no suspend):
  https://blog.mbirth.uk/2026/11/11/taming-the-cardputerzero.html
- AuraCam camera/I2C notes (IMX219 via Picamera2, I2C1 sharing, ext_5v_out, Grove UART): https://github.com/jrdntnnr/AuraCam
- Corroboration (1750 mAh, power, prices): https://shop.mtoolstec.com/product/m5stack-cardputerzero ,
  https://www.kickstarter.com/projects/m5stack/cardputerzero , https://atomiccreations.co/blogs/news/cardputer-adv-vs-cardputerzero

<!-- hw-scan:begin -->
_No device scan recorded yet. Run `/update-hwinfo` on the device to fill this
section with the measured hardware state._
<!-- hw-scan:end -->
