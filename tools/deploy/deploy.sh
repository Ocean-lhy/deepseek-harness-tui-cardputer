#!/bin/sh
# Cross-compile dst on this macOS host and install it on a Cardputer Zero.
#
# The device (Raspberry Pi CM0, aarch64, Debian 13, 446 MB RAM) OOM-kills a
# release build, so nothing is built there any more. This script builds on the
# host with the zig linker wrapper, verifies the aarch64 ELF, copies it over
# SSH, installs it with sudo and restarts cpzterm.service.
#
#   tools/deploy/deploy.sh [user@host] [remote-source-dir]
#
#   user@host           SSH target. There is no default device.
#                       Pass user@host or set DST_SSH_TARGET.
#                       hanxiao is the upstream author, not a device login.
#   remote-source-dir   IGNORED. Kept only for backwards compatibility: the old
#                       script rsynced sources there and built on the device.
#                       Cross-compiling no longer needs device-side sources.
#
# Flags (before the positional arguments):
#   -h, --help          show this help
#   --build-only        cross-compile + verify the artifact only; never SSH
#   --no-restart        install the binary but do not restart cpzterm.service
#
# Environment:
#   DST_SSH_PASSWORD    password auth via sshpass; also used for remote sudo
#   DST_SSH_KEY         SSH private key (if set with a password, both are used)
#   DST_NO_RESTART=1    same as --no-restart
#   DST_BUILD_ONLY=1    same as --build-only
#   DST_SSH_TARGET      same as the user@host argument
#   DST_CARGO_TARGET_DIR  staging dir (default /tmp/aarch64-zbuild)
#   DST_ZIG_CC          linker wrapper (default /tmp/zigcc/zig-cc)
#   DST_JOBS            cargo jobs (default 1; LTO + codegen-units=1 is serial)
#   DST_REMOTE_TMP      upload path (default /tmp/dst.new)
#   DST_REMOTE_BIN      install path (default /usr/local/bin/dst)
#   DST_SERVICE         systemd unit (default cpzterm.service)
#   DST_RESTART_WAIT    seconds to wait for the unit to become active (10)
#
# Host prerequisites - the script only detects them and prints the fix, it
# never installs software:
#   rustup target add aarch64-unknown-linux-gnu
#   brew install zig            # provides the aarch64 linker, wrapper in /tmp
#   brew install sshpass        # only needed for DST_SSH_PASSWORD
#
# Safe device diagnostics that do not take over the panel (they print and
# exit before the TUI starts):
#   /usr/local/bin/dst --tools
#   /usr/local/bin/dst --login-page [lang]
set -eu

RUST_TARGET=aarch64-unknown-linux-gnu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

usage() {
    sed -n '2,/^set -eu$/p' "$0" | sed -e 's/^# \{0,1\}//' -e '/^set -eu$/d' | sed -e '1d'
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
warn() { printf 'warning: %s\n' "$*" >&2; }
say() { printf '%s\n' "$*"; }

# ---- arguments --------------------------------------------------------------
BUILD_ONLY=${DST_BUILD_ONLY:-0}
NO_RESTART=${DST_NO_RESTART:-0}
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        --build-only) BUILD_ONLY=1; shift ;;
        --no-restart) NO_RESTART=1; shift ;;
        --) shift; break ;;
        -*) die "unknown option: $1 (try --help)" ;;
        *) break ;;
    esac
done
case "$BUILD_ONLY" in 1|true|yes) BUILD_ONLY=1 ;; *) BUILD_ONLY=0 ;; esac
case "$NO_RESTART" in 1|true|yes) NO_RESTART=1 ;; *) NO_RESTART=0 ;; esac

TARGET=${1:-${DST_SSH_TARGET:-}}
# Backwards compatibility: the old script's second positional argument was the
# device-side source directory. Sources are no longer copied, so ignore it.
REMOTE_SOURCE_DIR=${2:-}

CARGO_TARGET_DIR=${DST_CARGO_TARGET_DIR:-/tmp/aarch64-zbuild}
ZIG_CC=${DST_ZIG_CC:-/tmp/zigcc/zig-cc}
REMOTE_TMP=${DST_REMOTE_TMP:-/tmp/dst.new}
REMOTE_BIN=${DST_REMOTE_BIN:-/usr/local/bin/dst}
SERVICE=${DST_SERVICE:-cpzterm.service}
RESTART_WAIT=${DST_RESTART_WAIT:-10}

case "$REMOTE_TMP$REMOTE_BIN$SERVICE" in
    *[[:space:]]*) die "DST_REMOTE_TMP/DST_REMOTE_BIN/DST_SERVICE must not contain whitespace" ;;
esac
case "$RESTART_WAIT" in ''|*[!0-9]*) die "DST_RESTART_WAIT must be a number of seconds" ;; esac
[ -f "$HERE/Cargo.toml" ] || die "cannot find Cargo.toml at $HERE (run this script from the repo)"

# ---- stage 1: host toolchain ------------------------------------------------
# rustup lives outside the default non-interactive PATH on macOS.
export PATH="${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:${PATH}"

say "== check host toolchain"
command -v cargo >/dev/null 2>&1 || \
    die "cargo not found; install rustup or add it to PATH: export PATH=\"\$HOME/.cargo/bin:\$PATH\""
command -v rustc >/dev/null 2>&1 || \
    die "rustc not found; install rustup or add it to PATH: export PATH=\"\$HOME/.cargo/bin:\$PATH\""
say "   $(rustc --version)"
say "   $(cargo --version)"

if command -v rustup >/dev/null 2>&1; then
    if ! rustup target list --installed 2>/dev/null | grep -qx "$RUST_TARGET"; then
        die "Rust target $RUST_TARGET is not installed; run: rustup target add $RUST_TARGET"
    fi
else
    SYSROOT=$(rustc --print sysroot)
    [ -d "$SYSROOT/lib/rustlib/$RUST_TARGET/lib" ] || \
        die "Rust target $RUST_TARGET is not installed; run: rustup target add $RUST_TARGET"
fi
say "   target $RUST_TARGET: installed"

# The cross linker is zig's clang driver behind a tiny wrapper (zig knows the
# aarch64-linux-gnu sysroot, so no homebrew cross toolchain is needed).
if [ ! -x "$ZIG_CC" ]; then
    ZIG=${DST_ZIG:-$(command -v zig 2>/dev/null || true)}
    [ -n "$ZIG" ] || die "no aarch64 linker: $ZIG_CC is missing and zig is not on PATH; install it with: brew install zig"
    [ -x "$ZIG" ] || die "DST_ZIG=$ZIG is not an executable file; install zig with: brew install zig"
    say "   creating linker wrapper $ZIG_CC -> $ZIG"
    mkdir -p "$(dirname "$ZIG_CC")" || \
        die "cannot create the linker wrapper directory $(dirname "$ZIG_CC")"
    {
        printf '#!/bin/sh\n'
        printf '# zig acting as a C compiler/driver for %s\n' "$RUST_TARGET"
        printf 'exec %s cc -target aarch64-linux-gnu "$@"\n' "$ZIG"
    } > "$ZIG_CC" || die "cannot write the linker wrapper $ZIG_CC"
    chmod +x "$ZIG_CC"
fi
[ -x "$ZIG_CC" ] || die "$ZIG_CC is not executable; install zig with: brew install zig"
LINKER_INFO=$("$ZIG_CC" --version 2>/dev/null | head -1 || true)
say "   linker: $ZIG_CC${LINKER_INFO:+ ($LINKER_INFO)}"
if command -v zig >/dev/null 2>&1; then
    say "   zig: $(zig version 2>/dev/null || echo unknown)"
fi

# ---- stage 2: cross build ---------------------------------------------------
BIN="$CARGO_TARGET_DIR/$RUST_TARGET/release/dst"
say "== build dst for $RUST_TARGET (on this host; never on the device)"
say "   CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=$ZIG_CC"
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$ZIG_CC" \
CARGO_BUILD_JOBS="${DST_JOBS:-1}" \
    cargo build --release --target "$RUST_TARGET" --target-dir "$CARGO_TARGET_DIR" \
    --manifest-path "$HERE/Cargo.toml" --bin dst

# ---- stage 3: verify the artifact ------------------------------------------
[ -f "$BIN" ] || die "build reported success but $BIN is missing"
BYTES=$(wc -c < "$BIN" | tr -d ' \n')
HUMAN=$(ls -lh "$BIN" | awk '{print $5}')
FILE_INFO=$(file -b "$BIN")
say "== verify artifact"
say "   path:  $BIN"
say "   size:  $HUMAN ($BYTES bytes; expected ~1.3 MB for this crate)"
say "   file:  $FILE_INFO"
case "$FILE_INFO" in
    *ELF*64-bit*ARM*aarch64*) ;;
    *) die "artifact is not an aarch64 ELF binary: $FILE_INFO" ;;
esac
[ "$BYTES" -gt 100000 ] || warn "artifact is smaller than 100 KB; check the build output"
[ "$BYTES" -lt 8388608 ] || warn "artifact is larger than 8 MB; it is still installable but big for the device"

# readelf ships with binutils, which is not present on every macOS host; fall
# back to decoding the ELF header with od so the check always runs.
if command -v readelf >/dev/null 2>&1; then
    ELF_HDR=$(readelf -h "$BIN")
    case "$ELF_HDR" in *AArch64*) ;; *) die "readelf: artifact is not AArch64" ;; esac
    case "$ELF_HDR" in *DYN*) ;; *) warn "readelf: artifact is not position independent (expected Type DYN)" ;; esac
    printf '%s\n' "$ELF_HDR" | grep -E 'Class:|Type:|Machine:' | sed 's/^/   /' || true
else
    ELF_MAGIC=$(od -An -t x1 -N 4 "$BIN" | tr -d ' \n')
    ELF_CLASS=$(od -An -t u1 -j 4 -N 1 "$BIN" | tr -d ' \n')
    ELF_DATA=$(od -An -t u1 -j 5 -N 1 "$BIN" | tr -d ' \n')
    ELF_TYPE_LO=$(od -An -t u1 -j 16 -N 1 "$BIN" | tr -d ' \n')
    ELF_TYPE_HI=$(od -An -t u1 -j 17 -N 1 "$BIN" | tr -d ' \n')
    ELF_MACH_LO=$(od -An -t u1 -j 18 -N 1 "$BIN" | tr -d ' \n')
    ELF_MACH_HI=$(od -An -t u1 -j 19 -N 1 "$BIN" | tr -d ' \n')
    [ "$ELF_MAGIC" = 7f454c46 ] || die "artifact does not start with the ELF magic"
    [ "$ELF_CLASS" = 2 ] || die "artifact is not ELF64"
    [ "$ELF_DATA" = 1 ] || die "artifact is not little-endian"
    [ "$ELF_MACH_LO" = 183 ] && [ "$ELF_MACH_HI" = 0 ] || die "artifact e_machine is not AArch64 (0xb7): $ELF_MACH_HI$ELF_MACH_LO"
    say "   elf:   ELF64 LSB, e_machine=AArch64 (0xb7), e_type=$ELF_TYPE_HI$ELF_TYPE_LO (3=PIE/DYN)"
fi

if command -v shasum >/dev/null 2>&1; then
    LOCAL_SHA=$(shasum -a 256 "$BIN" | awk '{print $1}')
else
    LOCAL_SHA=$(sha256sum "$BIN" | awk '{print $1}')
fi
say "   sha256:$LOCAL_SHA"

if [ "$BUILD_ONLY" = 1 ]; then
    say "== build-only: skipping device copy/install"
    say "   artifact is ready at $BIN; rerun without --build-only to deploy"
    exit 0
fi

# ---- stage 4: device reachability + copy -----------------------------------
if [ -z "$TARGET" ]; then
    die "no SSH target. Pass user@host or set DST_SSH_TARGET. There is no default device. Use --build-only to compile without a target."
fi
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3"
SCP_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"
KEY_ARGS=""
if [ -n "${DST_SSH_KEY:-}" ]; then
    [ -f "$DST_SSH_KEY" ] || die "DST_SSH_KEY=$DST_SSH_KEY does not exist"
    case "$DST_SSH_KEY" in *" "*) die "DST_SSH_KEY must not contain spaces: $DST_SSH_KEY" ;; esac
    KEY_ARGS="-i $DST_SSH_KEY -o IdentitiesOnly=yes"
fi
# The device sudo needs the same password as SSH. It is base64-encoded and
# piped to `sudo -S` on the device so odd characters cannot break the remote
# shell quoting. (This is quoting safety, not a secrecy boundary: prefer
# DST_SSH_KEY plus passwordless sudo if process-list exposure matters.)
SUDO_B64=""
if [ -n "${DST_SSH_PASSWORD:-}" ]; then
    command -v sshpass >/dev/null 2>&1 || \
        die "DST_SSH_PASSWORD needs sshpass; install it (brew install sshpass) or use DST_SSH_KEY"
    command -v base64 >/dev/null 2>&1 || die "base64 not found; needed to pass the sudo password"
    SUDO_B64=$(printf '%s' "$DST_SSH_PASSWORD" | base64 | tr -d '\r\n')
    case "$SUDO_B64" in
        ''|*[!A-Za-z0-9+/=]*) die "could not base64-encode DST_SSH_PASSWORD" ;;
    esac
    export SSHPASS="$DST_SSH_PASSWORD"
    # Unquoted $SSH_OPTS/$KEY_ARGS are intentional: they are option lists.
    ssh_run() { sshpass -e ssh $SSH_OPTS $KEY_ARGS "$@"; }
    scp_run() { sshpass -e scp $SCP_OPTS $KEY_ARGS "$@"; }
else
    ssh_run() { ssh $SSH_OPTS $KEY_ARGS "$@"; }
    scp_run() { scp $SCP_OPTS $KEY_ARGS "$@"; }
fi

say "== check device $TARGET"
DEVICE_CMD='uname -srm; cat /etc/debian_version 2>/dev/null || true; free -m 2>/dev/null | head -2 || true; systemctl is-active '"$SERVICE"' 2>/dev/null || true'
DEVICE_INFO=$(ssh_run "$TARGET" "$DEVICE_CMD") || \
    die "cannot reach $TARGET over SSH. The verified artifact is at $BIN (cpzterm was NOT touched). Rerun when the device is back, or use --build-only."
printf '%s\n' "$DEVICE_INFO" | sed 's/^/   /'

say "== copy artifact to $TARGET:$REMOTE_TMP"
scp_run "$BIN" "$TARGET:$REMOTE_TMP"
REMOTE_SHA=$(ssh_run "$TARGET" "sha256sum '$REMOTE_TMP' 2>/dev/null | awk '{print \$1}'" || true)
if [ -n "$REMOTE_SHA" ]; then
    [ "$REMOTE_SHA" = "$LOCAL_SHA" ] || \
        die "sha256 mismatch after copy (host $LOCAL_SHA, device $REMOTE_SHA)"
    say "   sha256 verified on device"
else
    warn "sha256sum unavailable on the device; skipping integrity compare"
fi

# ---- stage 5: install ------------------------------------------------------
remote_sudo() {
    # $* is one shell command string; paths are validated to have no spaces.
    if [ -n "$SUDO_B64" ]; then
        # Password over the SSH channel, read by `sudo -S` on the device.
        ssh_run "$TARGET" "printf '%s' '$SUDO_B64' | base64 -d | sudo -S -p '' $*"
    elif ssh_run "$TARGET" "sudo -n true" >/dev/null 2>&1; then
        ssh_run "$TARGET" "sudo -n $*"
    else
        die "sudo on $TARGET needs a password; set DST_SSH_PASSWORD (the script pipes it to sudo -S) or enable passwordless sudo"
    fi
}

say "== install $REMOTE_BIN"
remote_sudo "install -m755 '$REMOTE_TMP' '$REMOTE_BIN'"
say "   $(ssh_run "$TARGET" "ls -l '$REMOTE_BIN'" || true)"

# ---- stage 6: restart and confirm the panel service ------------------------
STATE="not restarted (--no-restart)"
if [ "$NO_RESTART" = 1 ]; then
    say "== --no-restart: $SERVICE left running"
    say "   finish manually with: sudo systemctl restart $SERVICE"
else
    say "== restart $SERVICE"
    remote_sudo "systemctl restart '$SERVICE'"
    say "   waiting up to ${RESTART_WAIT}s for $SERVICE to become active"
    STATE=$(ssh_run "$TARGET" "i=0; while [ \$i -lt $RESTART_WAIT ]; do sleep 1; i=\$((i+1)); [ \"\$(systemctl is-active '$SERVICE' 2>/dev/null)\" = active ] && break; done; systemctl is-active '$SERVICE' 2>/dev/null || true" || true)
    STATE=$(printf '%s' "$STATE" | tr -d '\r')
    case "$STATE" in
        active) say "   $SERVICE: active" ;;
        *)
            warn "$SERVICE is '$STATE' after restart"
            ssh_run "$TARGET" "journalctl -u '$SERVICE' -n 30 --no-pager 2>/dev/null || true" | sed 's/^/   /' || true
            die "$SERVICE failed to become active; the panel may be down"
            ;;
    esac
fi

# ---- stage 7: smoke check (safe diagnostics) -------------------------------
say "== smoke check: dst --tools (prints the tool list and exits; does not take over the panel)"
TOOLS_OUT=$(ssh_run "$TARGET" "timeout 20 '$REMOTE_BIN' --tools 2>&1 || true") || TOOLS_OUT=""
printf '%s\n' "$TOOLS_OUT" | sed 's/^/   /'
case "$TOOLS_OUT" in
    *"tools="*) say "   --tools ok" ;;
    *) warn "unexpected --tools output; inspect the panel with: sudo systemctl status $SERVICE" ;;
esac

say "== done"
say "   installed $REMOTE_BIN ($HUMAN, sha256 $LOCAL_SHA)"
say "   $SERVICE: $STATE"
if [ "$NO_RESTART" = 1 ]; then
    say "   note: --no-restart was used, so the running panel still uses the previous binary"
fi
say "   safe diagnostics that exit without taking the panel: dst --tools, dst --login-page [lang]"
say "   if the panel stays dark: ssh $TARGET 'sudo journalctl -u $SERVICE -b --no-pager | tail -40'"
