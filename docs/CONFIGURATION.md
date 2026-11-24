# Configuration reference

`dst` keeps all of its state under `~/.dst`. Configuration is a single YAML
file that follows the DeepSeek Harness patch dialect: plain keys plus
`insert:` rows that shape individual tools.

## Where the file lives

| layer | path | notes |
| --- | --- | --- |
| system | `/etc/dst/config.yml` | optional, read first |
| user | `~/.dst/config.yml` | wins over the system layer; written by `/config` and `/login` |

`/config path` prints the user file. It is created with mode `600` because it
holds the credential.

## Annotated example

```yaml
model: deepseek-flash         # any model id the endpoint advertises
effort: high                  # auto | low | high | max
max_tokens: 4096

api:
  base: https://api.deepseek.com/v1
  key: sk-...                 # API key (mode 600); replaced by dsh_token after /login
  # dsh_token: ...            # DeepSeek Platform token -> x-dsh-auth-token
  retries: 2                  # transient-failure retries per request
  fallback_models: [deepseek-flash]

update:
  channel: nightly            # nightly (rolling main builds) | stable (vX.Y.Z releases)

voice:
  tts_voice: stella           # stella | tide

tui:
  queued_default: true        # typing during a turn queues instead of interrupting
  auto_send_voice: true       # empty prompt: send on release; with text: insert as dictation
  hw_survey_done: false       # first run offered the hardware-info scan
  language: en                # en | zh | es | ja | de | it
  mode: normal                # normal | creator

agent:
  max_steps: 0                # model requests per turn; 0 = unlimited (DSH default)

goal:
  max_rounds: 256             # continuation rounds per goal; 0 = unlimited
  max_steps: 0                # steps inside one goal round; 0 = unlimited
  judge: true                 # model judges complete/blocked via update_goal

compaction:
  enabled: true
  threshold_tokens: 512000    # absolute pressure cap
  keep_messages: 2            # minimum trailing messages kept verbatim
  max_tokens: 8192            # output budget of the summary call
  # model: deepseek-flash     # optional separate summariser model

tools:
  call_timeout_ms: 60000      # default executor deadline

context:
  time: true                  # inject the attributed clock reading
  time_refresh_secs: 600      # 0 = inject on every step

pet:
  enabled: false              # whale girl. false keeps the 53-column transcript
  dock: true                  # right-hand 10-column dock when the pet is on
  room: true                  # full idle room instead of the character-whale splash
  render: auto                # auto | pixel | text
  # fb: /dev/fb_lcd           # optional explicit panel; still must be 16bpp 320x170

skills:
  disabled: []                # skill names hidden from the catalog and the skill tool

sandbox:
  mode: auto                  # auto | strict | full
  judge_model: ""             # model for the auto-mode judge; empty = session model
  judge_max_tokens: 48        # verdict budget, minimum 8

# DSH-style patch rows: shape a tool's config or switch it off
insert:
  - id: speak
    name: speak
    disabled: true
  - id: search
    name: search
    config: { max_results: 5 }
```

## Key reference

### Top level

| key | default | meaning |
| --- | --- | --- |
| `model` | `deepseek-flash` | model id sent in every request |
| `effort` | `auto` | reasoning effort: `auto`, `low`, `high`, `max` |
| `max_tokens` | `4096` | output budget per request |

### `api`

| key | default | meaning |
| --- | --- | --- |
| `base` | `https://api.deepseek.com/v1` | OpenAI-compatible base URL; must be HTTPS |
| `key` | empty | API key, sent as `Authorization: Bearer` |
| `dsh_token` | empty | DeepSeek Platform token from `/login`, sent as `x-dsh-auth-token` |
| `retries` | `2` | retries per request on transient failures |
| `fallback_models` | empty | models tried in order when the primary keeps failing |

Setting `key` clears `dsh_token`, so the newer credential is never shadowed.

### `tui` and `voice`

| key | default | meaning |
| --- | --- | --- |
| `tui.queued_default` | `true` | queue input while a turn runs (`Shift+Enter` overtakes) |
| `tui.auto_send_voice` | `true` | send on release when the prompt was empty; with text in the prompt the take is always inserted as dictation (no auto-send, no overwrite) |
| `tui.language` | `en` | UI language: `en`, `zh`, `es`, `ja`, `de`, `it` |
| `tui.mode` | `normal` | `normal` or `creator` (see `docs/CREATOR.md`) |
| `tui.tips_shown` | `false` | one-time first-run tips already shown |
| `tui.hw_survey_done` | `false` | first run offered to scan the hardware into the `hardware-info` skill (set once, either answer) |
| `voice.tts_voice` | `stella` | `stella` or `tide` |

### `update`

| key | default | meaning |
| --- | --- | --- |
| `update.channel` | `nightly` | `nightly` = rolling build replaced on every commit to `main` (SHA-256 compared against the running binary); `stable` = newest `vX.Y.Z` release (version compared). On startup dst checks the channel in the background and asks `[Y]es/[N]o` when a newer build exists; `/update` checks and installs on demand, `/update stable|nightly` switches the channel (saved here). Downloads are SHA-256 verified, the binary is replaced atomically in place and dst re-execs itself. |

### Budgets

| key | default | meaning |
| --- | --- | --- |
| `agent.max_steps` | `0` | steps per turn; `0` = unlimited |
| `goal.max_rounds` | `256` | goal continuation rounds; `0` = unlimited |
| `goal.max_steps` | `0` | steps per goal round; `0` = unlimited |
| `goal.judge` | `true` | model self-judges completion |
| `tools.call_timeout_ms` | `60000` | deadline for tools that declare none |
| `context.time` | `true` | inject the clock reading into the step context |
| `context.time_refresh_secs` | `600` | minimum gap between clock injections; `0` = every step |

### Compaction

| key | default | meaning |
| --- | --- | --- |
| `compaction.enabled` | `true` | automatic compaction at `agent/pre-step` |
| `compaction.threshold_tokens` | `512000` | absolute pressure cap; the window ratio and headroom still clamp it |
| `compaction.keep_messages` | `2` | minimum trailing messages kept verbatim |
| `compaction.max_tokens` | `8192` | output budget of the summary call |
| `compaction.model` | empty | summariser model; empty = the session model |

### Sandbox

| key | default | meaning |
| --- | --- | --- |
| `sandbox.mode` | `auto` | `auto`, `strict`, `full` |
| `sandbox.judge_model` | empty | model used to judge ambiguous calls; empty = session model |
| `sandbox.judge_max_tokens` | `48` | verdict budget, minimum 8 |

## Patch rows (`insert:`)

DSH composes plugin lists as rows of `{ id, name, config, disabled }`. dst uses
the same shape to target a tool by `name`:

```yaml
insert:
  - id: speak          # unique row id
    name: speak        # tool to patch
    disabled: true     # removed from the registry and from the prompt
  - id: my-search
    name: search
    config:            # arbitrary per-tool config, handed to the tool
      max_results: 5
```

## Home layout (`~/.dst`)

```
~/.dst/
  config.yml     settings + credential (mode 600)
  sessions/      resumable conversation logs (<timestamp>.json, title inside)
  checkpoints/   turn checkpoints for /rewind
  skills/        skill packages: <name>/SKILL.md, optional meta.json, .history/vN.md
  mcp.json       MCP server definitions
  jobs.json      cron and file-watch jobs
  usage.json     cumulative token totals for /usage
  history        input history (mode 600)
  pet.json       whale-girl growth (mode 600); not an API key and not the chat
  pet-export.json / pet-export.wm1   written by /pet export
  pet-pack/      optional PPM sprites (idle.ppm and other poses)
  cache/         transient audio and scratch files
  logs/          voice.log, mcp.log
  tmp/           per-run temporary files, app_test logs, saved tool output
```

## Endpoint capabilities

The DeepSeek official API exposes speech (`/v1/tools/asr`, `/v1/tools/tts`) and
the server-side web tools (`search`, `fetch`, `ocr`). On any other base URL those
tools are removed from the registry, the model is told they are unavailable,
and the `/config` wizard warns before you switch. `dst --tools` prints the
effective set and the capability decision:

```text
$ dst --tools
base=https://api.deepseek.com/v1
official=true
web_tools=true speech=true
mode=normal
tools=artifact,bash,create_goal,...
```

## Environment and test hooks

Self-update hooks: `DST_NO_UPDATE_CHECK=1` disables the startup check;
`DST_UPDATE_BASE` points the updater at another mirror (tests); `DST_UPDATE_FORCE=1`
enables `/update` in developer builds (which default to "update disabled").

These exist for the simulator and the live walkthrough. None is set on the
device:

| variable | used by |
| --- | --- |
| `DST_MOCK_SPEECH=1` | fakes the recorder/ASR and logs TTS instead of playing |
| `DST_MOCK_ASR_TEXT` | transcript returned by the fake ASR |
| `DST_MOCK_TTS_MS` | fake TTS playback duration |
| `DST_BIN` | binary under test for `tools/sim` |
| `DST_NO_UPDATE_CHECK=1` | do not let a release binary replace itself; required for simulator runs |
| `DST_PET_FB_OUT` | write the pet's 320×170 PPM here (magenta is transparent) |
| `DST_FB` | explicit framebuffer path, overriding `pet.fb` |
| `DST_PET_FB_FORCE=1` | paint a probed panel even when fbterm is not an ancestor |
| `DST_LIVE_BASE`, `DST_LIVE_MODEL` | endpoint for `tools/sim/run_live.py` |

## Deployment variables (host side)

`tools/deploy/deploy.sh` cross-compiles on the host and pushes the binary over
SSH; nothing is built on the device (a release build on the 446 MB box gets
OOM-killed).

| variable | default | meaning |
| --- | --- | --- |
| `DST_SSH_KEY` | empty | SSH private key (`IdentitiesOnly`) |
| `DST_SSH_PASSWORD` | empty | password auth via `sshpass`; also piped to remote `sudo -S` |
| `DST_BUILD_ONLY=1` | `0` | cross-compile + verify the ELF, never SSH (same as `--build-only`) |
| `DST_NO_RESTART=1` | `0` | install the binary but leave `cpzterm.service` running (same as `--no-restart`) |
| `DST_CARGO_TARGET_DIR` | `/tmp/aarch64-zbuild` | cargo target dir for the cross build |
| `DST_ZIG_CC` | `/tmp/zigcc/zig-cc` | zig linker wrapper for `aarch64-unknown-linux-gnu` |
| `DST_JOBS` | `1` | cargo jobs (LTO + one codegen unit is serial anyway) |
| `DST_REMOTE_TMP` | `/tmp/dst.new` | upload path on the device |
| `DST_REMOTE_BIN` | `/usr/local/bin/dst` | install path on the device |
| `DST_SERVICE` | `cpzterm.service` | systemd unit restarted after install |
| `DST_RESTART_WAIT` | `10` | seconds to wait for the unit to become active |
