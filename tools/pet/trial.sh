#!/bin/sh
# Safe, local trial for the whale-girl pet. Run this ON the device.
# It does not SSH, does not install, does not write /usr/local/bin, and does
# not read or copy the real ~/.dst, keys, sessions or saves.
#
#   tools/pet/trial.sh --bin PATH --pack PATH [--fb /dev/fb_lcd] [--stage STAGE]
#
# STAGE is version, render, fb, or all (the default).
# There is no default user or host. The framebuffer node is a device path,
# not a machine address.
#
# Exit codes:
#   0   the requested stage finished
#   2   usage
#   10  another display program is using the panel; nothing was killed
#   11  could not back up the framebuffer
#   12  version or production render failed
#   13  the short framebuffer demo failed
#   14  the original framebuffer pixels could not be restored
set -u

BIN=""
PACK=""
FB="/dev/fb_lcd"
STAGE="all"

usage() {
    echo "usage: trial.sh --bin PATH --pack PATH [--fb /dev/fb_lcd] [--stage version|render|fb|all]" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bin) BIN=${2:-}; shift 2 ;;
        --pack) PACK=${2:-}; shift 2 ;;
        --fb) FB=${2:-}; shift 2 ;;
        --stage) STAGE=${2:-}; shift 2 ;;
        -h|--help) usage ;;
        *) usage ;;
    esac
done

[ -n "$BIN" ] && [ -n "$PACK" ] || usage
[ -x "$BIN" ] || { echo "trial: --bin is not executable: $BIN" >&2; exit 2; }
[ -f "$PACK/idle.ppm" ] || { echo "trial: --pack has no idle.ppm: $PACK" >&2; exit 2; }
case "$STAGE" in
    version|render|fb|all) ;;
    *) usage ;;
esac
case "$FB" in
    /dev/fb*) ;;
    *) echo "trial: --fb must be a /dev/fb* node, not a host" >&2; exit 2 ;;
esac

WORK=$(mktemp -d "${TMPDIR:-/tmp}/dst-pet-trial.XXXXXX") || exit 2
HOME="$WORK"
export HOME
export TMPDIR="$WORK"
export XDG_CONFIG_HOME="$WORK/xdg-config"
export XDG_DATA_HOME="$WORK/xdg-data"
export XDG_CACHE_HOME="$WORK/xdg-cache"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"
export DST_NO_UPDATE_CHECK=1
export DST_PET_TRIAL=1
export DST_PET_PACK="$PACK"
unset DST_UPDATE_FORCE OPENAI_API_KEY DEEPSEEK_API_KEY ANTHROPIC_API_KEY DST_API_KEY || true

LOG="$HOME/trial.log"
BACKUP=""
BACKUP_BYTES=0
RESTORED=0
touch "$LOG"

log() {
    printf '%s\n' "$*" | tee -a "$LOG" >&2
}

restore_fb() {
    stty sane 2>/dev/null || true
    if [ -n "$BACKUP" ] && [ -f "$BACKUP" ] && [ -e "$FB" ]; then
        if [ "$BACKUP_BYTES" -gt 0 ] && timeout 5 dd if="$BACKUP" of="$FB" bs="$BACKUP_BYTES" count=1 conv=notrunc status=none 2>>"$LOG"; then
            RESTORED=1
            log "trial: restored framebuffer from $BACKUP"
        else
            log "trial: FAILED to restore framebuffer from $BACKUP"
            RESTORED=0
        fi
    fi
}

trap 'restore_fb' EXIT INT TERM

log "trial: home=$HOME stage=$STAGE fb=$FB"
log "trial: no config, keys, sessions or saves were copied"

run_version() {
    log "trial: version"
    if ! "$BIN" --version >>"$LOG" 2>&1; then
        log "trial: version failed"
        return 12
    fi
    return 0
}

run_render() {
    log "trial: production render (no framebuffer)"
    OUT="$HOME/render"
    if ! "$BIN" --pet-demo "$OUT" >>"$LOG" 2>&1; then
        log "trial: pet-demo render failed"
        return 12
    fi
    if [ ! -s "$OUT/idle.ppm" ] || [ ! -s "$OUT/text-idle.txt" ]; then
        log "trial: render did not write idle.ppm and text-idle.txt"
        return 12
    fi
    log "trial: render ok $OUT"
    return 0
}

display_busy() {
    realfb=$(readlink -f "$FB" 2>/dev/null || printf '%s' "$FB")
    hits="$HOME/display-hits.txt"
    : >"$hits"
    ps -eo pid=,comm= >"$HOME/ps.txt" 2>/dev/null || true
    while read -r pid comm; do
        case "$comm" in
            fbterm|Xorg|weston|sway|wayfire|cage|dst)
                printf '%s %s\n' "$pid" "$comm" >>"$hits"
                ;;
        esac
    done <"$HOME/ps.txt"
    for fd in /proc/[0-9]*/fd/*; do
        target=$(readlink "$fd" 2>/dev/null || true)
        case "$target" in
            "$FB"|"$realfb")
                printf '%s -> %s\n' "$fd" "$target" >>"$hits"
                ;;
        esac
    done
    if [ -s "$hits" ]; then
        log "trial: other display users found; not drawing and not killing them:"
        cat "$hits" | tee -a "$LOG" >&2
        return 0
    fi
    return 1
}

run_fb() {
    if [ ! -e "$FB" ]; then
        log "trial: framebuffer node missing: $FB"
        return 11
    fi
    if display_busy; then
        return 10
    fi
    node=$(basename "$realfb")
    sys="/sys/class/graphics/$node"
    stride=$(cat "$sys/stride" 2>/dev/null || echo 640)
    bits=$(cat "$sys/bits_per_pixel" 2>/dev/null || echo 16)
    virtual=$(cat "$sys/virtual_size" 2>/dev/null || echo "320,170")
    height=$(printf '%s' "$virtual" | awk -F, '{print $2}')
    [ -n "$height" ] || height=170
    log "trial: panel node=$node stride=$stride bits=$bits virtual=$virtual"
    if [ "$bits" != 16 ] || [ "$height" != 170 ] || [ "$stride" -lt 640 ]; then
        log "trial: refusing to back up a panel that is not 16bpp 170 rows stride>=640"
        return 11
    fi
    bytes=$((stride * height))
    BACKUP="$HOME/fb-before.bin"
    BACKUP_BYTES=$bytes
    if ! timeout 5 dd if="$FB" of="$BACKUP" bs="$bytes" count=1 status=none 2>>"$LOG"; then
        log "trial: framebuffer backup failed"
        return 11
    fi
    got=$(wc -c <"$BACKUP" | tr -d ' ')
    if [ "$got" != "$bytes" ]; then
        log "trial: backup size $got is not $bytes"
        return 11
    fi
    log "trial: backed up $bytes bytes to $BACKUP"
    export DST_FB="$FB"
    code=0
    timeout 15 "$BIN" --pet-demo "$HOME/fb-demo" --fb >>"$LOG" 2>&1 || code=$?
    if [ "$code" != 0 ]; then
        log "trial: framebuffer demo exited $code"
        return 13
    fi
    log "trial: framebuffer demo finished"
    return 0
}

rc=0
case "$STAGE" in
    version) run_version || rc=$? ;;
    render) run_render || rc=$? ;;
    fb) run_fb || rc=$? ;;
    all)
        run_version || rc=$?
        if [ "$rc" -eq 0 ]; then
            run_render || rc=$?
        fi
        if [ "$rc" -eq 0 ]; then
            run_fb || rc=$?
        fi
        ;;
esac

if [ -n "$BACKUP" ] && [ -f "$BACKUP" ]; then
    restore_fb
    trap - EXIT INT TERM
    if [ "$RESTORED" != 1 ] && [ "$rc" -eq 0 ]; then
        rc=14
    fi
fi

log "trial: exit $rc log=$LOG"
if [ "$rc" -eq 0 ]; then
    printf '%s\n' "$LOG"
fi
exit "$rc"
