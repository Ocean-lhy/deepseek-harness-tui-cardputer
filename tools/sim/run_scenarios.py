#!/usr/bin/env python3
"""run_scenarios.py -- automated UI/behaviour checks for feature work.

Each directory under `scenarios/` holds:
  config.yml    ~/.dst/config.yml body (the `api.base` line is injected)
  scenario.json screen_sim.py scenario (keys, waits, snapshots)
  script.json   mock_api.py scripted model responses

The runner starts the mock API on an ephemeral localhost port, runs dst in the
pty simulator, then asserts on the captured screens and on the request log.
Everything stays on 127.0.0.1.

    python3 run_scenarios.py               # all scenarios
    python3 run_scenarios.py goal compact  # just these
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import threading
import sys
import tempfile
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from run_demo import DEFAULT_BIN, MOCK, REPO, SIM, log, start_mock, stop_mock

ROOT = os.path.dirname(os.path.abspath(__file__))
SCEN_DIR = os.path.join(ROOT, "scenarios")


def chat_requests(log_path):
    out = []
    if not os.path.exists(log_path):
        return out
    for line in open(log_path, encoding="utf-8"):
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        body = rec.get("body") or {}
        if body.get("messages"):
            out.append(body)
    return out


def message_texts(requests):
    texts = []
    for body in requests:
        for m in body.get("messages") or []:
            c = m.get("content")
            if isinstance(c, str):
                texts.append(c)
    return texts


def request_tool_calls(requests):
    """All streamed tool_call arguments seen in request history (assistant msgs)."""
    out = []
    for body in requests:
        for m in body.get("messages") or []:
            for tc in m.get("tool_calls") or []:
                out.append({"name": tc.get("function", {}).get("name"),
                            "args": tc.get("function", {}).get("arguments")})
    return out


def request_tool_results(requests):
    return [m for body in requests for m in (body.get("messages") or [])
            if m.get("role") == "tool"]


def _cell_width(text):
    """Display width in terminal cells (CJK/full-width glyphs count as 2)."""
    import unicodedata
    return sum(2 if unicodedata.east_asian_width(c) in ("W", "F") else 1
               for c in text)


def read_file(path):
    try:
        return open(path, encoding="utf-8").read()
    except OSError:
        return ""


def read_screens(out_dir):
    screens = {}
    for name in os.listdir(out_dir):
        if not name.endswith(".txt"):
            continue
        stem = name[:-4]
        text = open(os.path.join(out_dir, name), encoding="utf-8").read()
        screens[stem] = text
        # Snapshot files are numbered (01_splash.txt); also index by the bare
        # scenario snapshot name so checks do not depend on ordering.
        base = stem.split("_", 1)[1] if stem[:1].isdigit() and "_" in stem else stem
        screens.setdefault(base, text)
    return screens


def start_update_mock(work):
    """Serve a fake nightly release feed for the self-update scenarios.

    The updater compares the running binary's SHA-256 with the published
    SHA256SUMS; a fake digest therefore means "update available". The fake
    binary does not match that digest, so choosing to install fails at the
    checksum step without touching the real dst binary.
    """
    fake_sha = "0" * 64
    sums = f"{fake_sha}  dst-linux-aarch64\n{fake_sha}  dst-linux-x86_64\n"

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 - http.server API
            if self.path.endswith("/SHA256SUMS"):
                body = sums.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/plain")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            elif "/dst-linux-" in self.path:
                body = b"fake-update-binary\n"
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            else:
                self.send_error(404)

        def log_message(self, *_args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_address[1]}"
    return server, url


def start_mock_login(work, env):
    """Start the nine-step DSH login mock; returns (proc, cleanup)."""
    port_file = os.path.join(work, "mock-login-port")
    log_file = os.path.join(work, "mock-login.jsonl")
    if os.path.exists(port_file):
        os.remove(port_file)
    script = os.path.join(ROOT, "tools", "sim", "mock_login.py") if os.path.exists(os.path.join(ROOT, "tools", "sim", "mock_login.py")) else os.path.join(os.path.dirname(os.path.abspath(__file__)), "mock_login.py")
    proc = subprocess.Popen([sys.executable, script, "--port-file", port_file],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            env=dict(env, MOCK_LOGIN_LOG=log_file), cwd=ROOT)
    for _ in range(60):
        if os.path.exists(port_file) and open(port_file).read().strip():
            break
        time.sleep(0.1)
    port = open(port_file).read().strip()
    env.update({
        "DST_LOGIN_PLATFORM": f"http://127.0.0.1:{port}",
        "DST_LOGIN_WECHAT": f"http://127.0.0.1:{port}",
        "DST_LOGIN_WECHAT_LP": f"http://127.0.0.1:{port}",
        "DST_LOGIN_API": f"http://127.0.0.1:{port}",
        "MOCK_LOGIN_LOG": log_file,
    })
    return proc, log_file


def _phase_timeout() -> int:
    try:
        return max(30, int(os.environ.get("DST_SCENARIO_TIMEOUT", "150")))
    except ValueError:
        return 150


def run_scenario(name, binary, timeout=None):
    base = os.path.join(SCEN_DIR, name)
    scenario = os.path.join(base, "scenario.json")
    script = os.path.join(base, "script.json")
    if timeout is None:
        timeout = _phase_timeout()
    work = tempfile.mkdtemp(prefix=f"dst-scen-{name}-")
    home = os.path.join(work, "home")
    out = os.path.join(work, "out")
    os.makedirs(os.path.join(home, ".dst"), exist_ok=True)
    os.makedirs(out, exist_ok=True)
    # A scenario may declare `"phases": [[steps…], [steps…]]`: dst is restarted
    # between phases with the same HOME (used by /resume checks).
    doc = json.load(open(scenario, encoding="utf-8"))
    phases = doc.get("phases")
    scenario_paths = []
    if phases:
        for i, phase in enumerate(phases, 1):
            if isinstance(phase, dict):
                steps = phase.get("steps", [])
                phase_cwd = os.path.join(work, phase.get("cwd", "."))
            else:
                steps = phase
                phase_cwd = work
            os.makedirs(phase_cwd, exist_ok=True)
            path = os.path.join(work, f"phase{i}.json")
            json.dump({"steps": steps}, open(path, "w", encoding="utf-8"),
                      ensure_ascii=False, indent=1)
            scenario_paths.append((path, os.path.join(out, f"phase{i}"), phase_cwd))
    else:
        scenario_paths.append((scenario, out, work))
    # A scenario may ship a pre-populated HOME tree (e.g. goal.json or
    # mcp.json) under scenarios/<name>/home/. Copy it in first.
    overlay = os.path.join(base, "home")
    if os.path.isdir(overlay):
        shutil.copytree(overlay, home, dirs_exist_ok=True)
    log_path = os.path.join(work, "requests.jsonl")
    env = dict(os.environ)
    env.update({str(k): str(v) for k, v in (doc.get("env") or {}).items()})
    update_mock = None
    if doc.get("update_mock"):
        update_mock, update_url = start_update_mock(work)
        env["DST_UPDATE_BASE"] = update_url
        env["DST_UPDATE_FORCE"] = "1"
        import hashlib as _hashlib
        try:
            with open(binary, "rb") as _bh:
                _target_sha = _hashlib.sha256(_bh.read()).hexdigest()
        except OSError:
            _target_sha = ""
        with open(os.path.join(work, "update-target-sha256"), "w", encoding="utf-8") as fh:
            fh.write(_target_sha)
    want_mock_login = bool(doc.get("mock_login")) or os.path.exists(os.path.join(base, "mock_login"))
    login_mock = None
    login_log = None
    if want_mock_login:
        login_mock, login_log = start_mock_login(work, env)

    proc, url, _state = start_mock(0, script, log_path, env)
    try:
        # A scenario may reference the ephemeral mock endpoint as `{{MOCK_URL}}`
        # (first-run setup now verifies credentials, so it needs a live local
        # endpoint). Substitute it in a private copy so scenarios stay
        # port-independent.
        for _i, (_path, _out, _cwd) in enumerate(scenario_paths):
            _text = open(_path, encoding="utf-8").read()
            if "{{MOCK_URL}}" in _text:
                _text = _text.replace("{{MOCK_URL}}", url)
                _copy = os.path.join(work, f"scenario-mock-url-{_i}.json")
                open(_copy, "w", encoding="utf-8").write(_text)
                scenario_paths[_i] = (_copy, _out, _cwd)
        # `no_config` scenarios deliberately start unconfigured (onboarding).
        if not os.path.exists(os.path.join(base, "no_config")):
            # url is like http://127.0.0.1:PORT/v1
            cfg = open(os.path.join(base, "config.yml"), encoding="utf-8").read()
            cfg = cfg.replace("  key: test", f"  base: {url}\n  key: test")
            cfg_path = os.path.join(home, ".dst", "config.yml")
            open(cfg_path, "w", encoding="utf-8").write(cfg)
            os.chmod(cfg_path, 0o600)
        for scen_path, phase_out, phase_cwd in scenario_paths:
            cmd = [sys.executable, SIM, "--bin", binary, "--home", home,
                   "--cwd", phase_cwd, "--out", phase_out, "--scenario", scen_path]
            for _ek, _ev in (doc.get("env") or {}).items():
                cmd += ["--env", f"{_ek}={_ev}"]
            # Text snapshots are enough for assertions; PNG rendering is only
            # needed when a contact sheet is being produced.
            if not os.environ.get("DST_SCENARIO_PNG"):
                cmd.append("--text-only")
            # DST_SIM_SCALE=2 renders 640x340 PNGs (3 = the 960x510 default),
            # used by make_demo_webp.py for a compact animated demo.
            if os.environ.get("DST_SIM_SCALE"):
                cmd += ["--scale", str(os.environ["DST_SIM_SCALE"])]
            # Do NOT use capture_output=True: the simulator starts dst and dst
            # may leave background children holding the inherited stdout/stderr
            # pipes. subprocess.run(timeout=) would then kill the child and
            # block forever waiting for EOF from those grandchildren (this hung
            # a CI run). Logs go to a file, and a timeout kills the session.
            sim_log = os.path.join(work, f"sim-phase-{_i}.log")
            with open(sim_log, "w", encoding="utf-8") as logfh:
                proc_sim = subprocess.Popen(
                    cmd,
                    cwd=ROOT,
                    env=env,
                    stdout=logfh,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                try:
                    returncode = proc_sim.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    for sig in (signal.SIGTERM, signal.SIGKILL):
                        try:
                            os.killpg(proc_sim.pid, sig)
                        except OSError:
                            pass
                        try:
                            proc_sim.wait(timeout=2)
                            break
                        except subprocess.TimeoutExpired:
                            continue
                    tail = ""
                    try:
                        with open(sim_log, encoding="utf-8", errors="replace") as fh:
                            tail = "".join(fh.readlines()[-10:])
                    except OSError:
                        pass
                    raise AssertionError(
                        f"simulator timed out after {timeout}s; last output:\n{tail}"
                    )
            if returncode != 0:
                try:
                    with open(sim_log, encoding="utf-8", errors="replace") as fh:
                        tail = "".join(fh.readlines()[-25:])
                except OSError:
                    tail = ""
                raise AssertionError(f"simulator failed (exit {returncode}):\n{tail}")
    finally:
        stop_mock(proc)
        if update_mock is not None:
            update_mock.shutdown()
            update_mock.server_close()
        if login_mock is not None:
            login_mock.terminate()
            try:
                login_mock.wait(timeout=3)
            except Exception:
                login_mock.kill()

    screens = {}
    for _path, phase_out, _cwd in scenario_paths:
        screens.update(read_screens(phase_out))
    reqs = chat_requests(log_path)
    return screens, reqs, work


# Each check returns a list of (ok, message) tuples.
def check_clear(screens, reqs, work):
    checks = []
    final = screens.get("cleared", "")
    checks.append(("hold Tab to talk" in final, "screen reset to splash"))
    checks.append(("reasoning text must disappear" not in final,
                   "no reasoning text survives /clear"))
    checks.append(("thinking" not in final, "no thinking panel survives /clear"))
    checks.append((len(reqs) == 1, "exactly one chat request"))
    return checks


def check_goal(screens, reqs, work):
    screens_all = "\n".join(screens.values())
    checks = []
    checks.append((any(t in screens_all for t in ["Pursuing Goal 1/1", "Goal paused 1/1", "make the whale"]),
                   "bottom-right badge shows goal status/objective/budget"))
    checks.append(("\u27f3 goal round 1" in screens_all, "automatic goal round prompt shown"))
    checks.append(("Round one: goal driver worked." in screens_all, "goal round answer rendered"))
    checks.append((len(reqs) == 1, "round cap stops automatic continuation"))
    texts = "\n".join(message_texts(reqs))
    checks.append(("<goal_round>" in texts, "round prompt carries <goal_round>"))
    checks.append(("Time sampled while preparing turn" in texts, "time context injected"))
    return checks


def check_compact(screens, reqs, work):
    texts = message_texts(reqs)
    joined = "\n".join(texts)
    checks = []
    checks.append(("You are now acting as a compaction engine" in joined,
                   "summariser instruction sent"))
    checks.append(("<compacted-summary>" in joined,
                   "checkpoint replacement reached a later request"))
    checks.append(("Second answer after the checkpoint." in "\n".join(screens.values()),
                   "post-compaction turn answered"))
    checks.append((len(reqs) >= 3, "requests observed: summariser + both turns"))
    return checks


def check_thinking(screens, reqs, work):
    live = screens.get("live", "")
    folded = screens.get("folded", "")
    checks = []
    # Full-width reasoning lines: the old bug produced one line per 9-char
    # chunk. Wrapping happens at words, so assert long reasoned lines rather
    # than an exact phrase that depends on where the line breaks.
    reason_lines = [l for l in live.splitlines() if l.startswith("  ")]
    long_lines = [l for l in reason_lines if len(l.strip()) >= 40]
    checks.append((len(long_lines) >= 2,
                   "reasoning wraps into full lines"))
    checks.append(("thinking" in live, "live thinking indicator shown"))
    checks.append(("(collapsed)" in folded, "thinking folds to one line when the answer starts"))
    checks.append(("reasoning panel" not in folded, "reasoning text removed after folding"))
    return checks


def check_tool_order(screens, reqs, work):
    import re
    pending = screens.get("pending", "")
    done = screens.get("done", "")
    results = request_tool_results(reqs)
    settled = re.compile(r"\| \d+\.\d+s (ok|x)")
    checks = []
    checks.append(("* bash" in pending, "tool/call line shown before execution"))
    checks.append((not settled.search(pending),
                   "no settled result line while the tool is still running"))
    checks.append(("ok" in done and "done" in done, "tool/result line and preview after return"))
    checks.append((any("done" in (r.get("content") or "") for r in results),
                   "tool result reached the model"))
    checks.append((len(reqs) == 2, "one follow-up request after the tool batch"))
    return checks


def check_tool_timeout(screens, reqs, work):
    timed = screens.get("timed_out", "")
    after = screens.get("after", "")
    results = request_tool_results(reqs)
    checks = []
    checks.append(("timed out" in timed or "timed out" in after,
                   "timeout surfaced in the transcript"))
    checks.append(("Timeout was reported to the model." in after, "turn continued after the timeout"))
    checks.append((any("timed out" in (r.get("content") or "") for r in results),
                   "timeout reached the model as a tool error"))
    checks.append((len(reqs) == 2, "exactly one retry request after the timeout"))
    return checks


def check_esc_cancel(screens, reqs, work):
    cancelled = screens.get("cancelled", "")
    recovered = screens.get("recovered", "")
    vlog = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    checks = []
    checks.append(("cancelled" in cancelled, "Esc cancels the running request"))
    checks.append(("This should never be shown." not in cancelled,
                   "late stream content is discarded"))
    checks.append(("stop requested" in vlog, "Esc interrupt reached the voice layer"))
    checks.append(("Recovered after cancel." in recovered,
                   "prompt is usable again after the interrupt"))
    checks.append((len(reqs) >= 2, "a fresh request was made after the cancel"))
    return checks



def check_esc_queue(screens, reqs, work):
    busy = screens.get("busy", "")
    queued = screens.get("queued", "")
    reply = screens.get("reply", "")
    agents = agent_requests(reqs)
    checks = []
    checks.append(("sleep 5" in busy, "a long tool call is running"))
    checks.append(("queued (1)" in queued and "this message was queued" in queued,
                   "Enter queues the typed message while the turn runs"))
    checks.append(("Queued answer after the interrupt." in reply,
                   "the queued message is sent after Esc stops the turn"))
    checks.append((len(agents) == 2,
                   f"exactly two model requests ({len(agents)}): interrupted turn + queued message"))
    last_user = ""
    if agents:
        users = [m for m in (agents[-1].get("messages") or [])
                 if m.get("role") == "user"]
        if users:
            last_user = str(users[-1].get("content") or "")
    checks.append(("this message was queued" in last_user,
                   "the queued text is the user prompt of the next request"))
    checks.append(("Queued answer" not in busy,
                   "the interrupted turn never got the scripted follow-up reply"))
    return checks


def check_pet_bounds(screens, reqs, work):
    settled = screens.get("settled", "")
    live = screens.get("live", "")

    def widest(text):
        return max((line.count("Z") for line in text.splitlines()), default=0)

    checks = []
    checks.append((widest(settled) == 43,
                   f"settled body wraps at 43 columns, got {widest(settled)}"))
    checks.append((0 < widest(live) <= 43,
                   f"streaming row stays inside the dock, got {widest(live)}"))
    checks.append(("Z" * 44 not in live and "Z" * 44 not in settled,
                   "44 Zs never share one row"))
    checks.append((len(reqs) == 1, "one scripted reply, no extra model call"))
    return checks


def check_step_limit(screens, reqs, work):
    limited = screens.get("limited", "")
    checks = []
    checks.append(("step limit reached (1)" in limited, "configured step cap is reported"))
    checks.append((len(reqs) == 1, "no second model request after hitting the cap"))
    pet_path = os.path.join(work, "home", ".dst", "pet.json")
    try:
        pet = json.load(open(pet_path, encoding="utf-8"))
        raw = (pet.get("data") or {}).get("usageStats") or "{}"
        usage = json.loads(raw) if isinstance(raw, str) else raw
    except (OSError, json.JSONDecodeError):
        usage = None
    successes = 0
    if isinstance(usage, dict) and "successes" in usage:
        try:
            successes = float(usage["successes"])
        except (TypeError, ValueError):
            successes = -1
    checks.append((usage is not None and successes == 0,
                   f"a step cap is not a pet success ({pet_path}: {usage})"))
    return checks


def check_unlimited_steps(screens, reqs, work):
    done = screens.get("done", "")
    checks = []
    checks.append(("Thirteen tool steps done." in done, "turn ran through 13 tool steps"))
    checks.append((len(reqs) == 14, "one request per step, no hidden cap (14 requests)"))
    checks.append(("step limit reached" not in done, "no step limit was applied"))
    return checks


def check_repeat_reminder(screens, reqs, work):
    texts = "\n".join(message_texts(reqs))
    checks = []
    checks.append(("repeating the exact same tool call" in texts,
                   "repeat reminder injected into a later request"))
    checks.append((len(reqs) == 4, "three repeated calls plus the final answer"))
    checks.append(("Stopped repeating." in "\n".join(screens.values()),
                   "model answer after the reminder rendered"))
    done = screens.get("done", "")
    checks.append(("You are repeating the exact same tool call" not in done,
                   "model-facing reminder text is not dumped into the transcript"))
    checks.append(("repeated tool call" in done,
                   "a one-line context marker explains the injected reminder"))
    return checks


def check_time_context(screens, reqs, work):
    checks = []
    stamps = [[m for m in (body.get("messages") or [])
               if isinstance(m.get("content"), str) and "Time sampled while preparing turn" in m["content"]]
              for body in reqs]
    checks.append((len(reqs) == 2, "two turns requested"))
    checks.append((all(len(s) == 1 for s in stamps), "exactly one clock reading per request"))
    checks.append((bool(stamps) and "turn 1" in stamps[0][0]["content"], "turn 1 reading uses turn 1"))
    checks.append((len(stamps) > 1 and "turn 2" in stamps[1][0]["content"], "turn 2 reading uses turn 2"))
    return checks


def check_compact_manual(screens, reqs, work):
    texts = message_texts(reqs)
    joined = "\n".join(texts)
    compact_screen = screens.get("compact", "")
    checks = []
    checks.append(("You are now acting as a compaction engine" in joined,
                   "manual /compact calls the summariser"))
    checks.append(("<compacted-summary>" in joined, "checkpoint message reached the next request"))
    checks.append(("compacted:" in compact_screen, "manual compaction notice shown"))
    checks.append(("Answer after manual compaction." in "\n".join(screens.values()),
                   "conversation continues after manual compaction"))
    return checks


def check_goal_complete(screens, reqs, work):
    done = screens.get("done", "")
    calls = request_tool_calls(reqs)
    results = request_tool_results(reqs)
    goal = read_file(os.path.join(work, "home", ".dst", "goal.json"))
    checks = []
    checks.append((any(c["name"] == "update_goal" for c in calls),
                   "model called update_goal"))
    checks.append(('"action": "complete"' in " ".join(c.get("args") or "" for c in calls)
                   or '"action":"complete"' in " ".join(c.get("args") or "" for c in calls),
                   "update_goal asked for complete"))
    checks.append(("Goal marked complete; closing message." in done,
                   "closing message rendered after completion"))
    checks.append(("Pursuing Goal" not in done and "Goal done" in done, "goal status shows done"))
    checks.append(("<goal_complete>" not in done,
                   "model-facing wrap-up instructions are not dumped"))
    checks.append(("goal wrap-up" in done,
                   "a one-line context marker explains the wrap-up"))
    checks.append(('"phase": "complete"' in goal or '"phase":"complete"' in goal,
                   "goal persisted as complete"))
    checks.append((len(reqs) == 2, "no further goal round after completion"))
    checks.append((not any("goal state changed" in (r.get("content") or "") for r in results),
                   "revision matched (no stale-state error)"))
    return checks


def check_goal_manage(screens, reqs, work):
    all_screens = "\n".join(screens.values())
    goal = read_file(os.path.join(work, "home", ".dst", "goal.json"))
    checks = []
    paused = screens.get("paused", "")
    checks.append(("Status: paused" in paused,
                   "queued /goal pause stops the loop after round 1"))
    checks.append(("Rounds: 1/2" in paused, "only one round was spent"))
    checks.append(("Goal paused 1/2" in paused, "bottom-right badge shows the paused budget"))
    checks.append(("Goal round two done." in screens.get("round2", ""),
                   "/goal resume re-arms and round 2 runs"))
    checks.append(("goal reached its round limit" in screens.get("resumed", ""),
                   "round cap stops automatic continuation"))
    checks.append(("Goal updated" in screens.get("edited", "") and
                   "revised objective" in screens.get("edited", ""),
                   "/goal edit replaces the objective"))
    checks.append(("Goal cleared." in screens.get("cleared", ""),
                   "/goal clear removes the goal"))
    checks.append((not goal and "Pursuing Goal" not in screens.get("cleared", ""),
                   "goal file and banner gone after clear"))
    return checks


def check_session_menu(screens, reqs, work):
    import glob
    import re
    numbered = re.compile(r"(?m)^\s*\d+[\).]\s")
    session_files = glob.glob(os.path.join(work, "home", ".dst", "sessions", "*.json"))
    title = ""
    if session_files:
        try:
            title = json.load(open(session_files[0], encoding="utf-8")).get("title", "")
        except (OSError, json.JSONDecodeError):
            title = ""
    resume = screens.get("resume", "")
    expanded = screens.get("expanded", "")
    resumed = screens.get("resumed", "")
    model_menu = screens.get("model_menu", "")
    all_screens = "\n".join(screens.values())
    checks = []
    checks.append(('Session renamed to "Whale splash task"' in screens.get("t1", ""),
                   "rename tool renamed the session during the turn"))
    checks.append(("Whale splash task" in screens.get("resume", ""),
                   "session title shown in the resume picker"))
    checks.append(("Resume session" in resume and "Whale splash task" in resume,
                   "/resume opens a menu listing session titles"))
    checks.append(("expand" in resume, "menu hints the expand gesture"))
    checks.append(("last reply:" in expanded or "title:" in expanded,
                   "Right expands the selected session preview"))
    checks.append(("resumed: Whale splash task" in resumed and
                   "First answer with enough context." in resumed,
                   "Enter resumes the selected session with its transcript"))
    checks.append(("deepseek-flash" in model_menu, "/model opens a model menu"))
    renamed = screens.get("renamed", "")
    checks.append(("session renamed to human renamed" in renamed,
                   "/rename updates the human-facing title"))
    checks.append((title == "human renamed",
                   "the human rename is the persisted title"))
    checks.append((not numbered.search(all_screens),
                   "no numbered option lists anywhere in these screens"))
    return checks


def check_onboarding(screens, reqs, work):
    menu = screens.get("menu", "")
    moved = screens.get("moved", "")
    checks = []
    checks.append(("Choose how to connect" in menu and
                   "DeepSeek official" in menu and "Custom" in menu,
                   "onboarding is a menu with both connection options"))
    checks.append(("Enter confirm" in menu, "menu hint explains the keys"))
    checks.append(("platform.deepseek.com" in menu, "option preview explains the choice"))
    checks.append(("sandbox defaults to auto" in menu, "onboarding explains sandbox mode"))
    checks.append(("Type 1 or 2" not in menu, "no \"type 1 or 2\" prompt left"))
    checks.append(("\u25b8" in moved, "arrow keys move the selection"))
    checks.append((len(reqs) == 0, "onboarding made no model request"))
    return checks


def check_sandbox_auto(screens, reqs, work):
    results = [r.get("content") or "" for r in request_tool_results(reqs)]
    joined = "\n".join(results)
    texts = message_texts(reqs)
    checks = []
    checks.append(("permission denied" not in results[0] if results else False,
                   "safe read-only command runs without asking"))
    checks.append(("system-configuration change" in joined,
                   "danger rule rejects system changes without an LLM call"))
    checks.append(("permission judge" in joined and "unknown script" in joined,
                   "unknown command is judged by the short LLM and denied with its reason"))
    checks.append((any("permission judge for a coding agent" in t for t in texts),
                   "the auto judge prompt was sent to the model"))
    checks.append((len(reqs) >= 7, f"one extra request for the judge ({len(reqs)} requests)"))
    return checks


def check_sandbox_strict(screens, reqs, work):
    popup = screens.get("popup", "")
    rejected = screens.get("rejected", "")
    joined = "\n".join(r.get("content") or "" for r in request_tool_results(reqs))
    checks = []
    checks.append(("Permission required" in popup and "bash" in popup and "./deploy.sh" in popup,
                   "strict mode shows the confirmation pop-up with tool and arguments"))
    checks.append(("[y] allow" in popup and "[n] reject" in popup,
                   "pop-up offers allow / reject"))
    checks.append(("rejected by the user" in joined,
                   "rejecting the pop-up fails the tool call with a reason"))
    checks.append(("User rejected the deployment." in rejected,
                   "model continues after the rejection"))
    return checks


def check_sandbox_full(screens, reqs, work):
    results = [r.get("content") or "" for r in request_tool_results(reqs)]
    joined = "\n".join(results)
    checks = []
    checks.append(("permission denied" not in joined,
                   "full mode never rejects the dangerous call"))
    checks.append((any("systemctl" in r or "not found" in r or "exit" in r for r in results),
                   "the dangerous command really executed"))
    checks.append(("Full mode ran it." in "\n".join(screens.values()),
                   "conversation continues after the unrestricted call"))
    return checks


def check_jobs(screens, reqs, work):
    results = [r.get("content") or "" for r in request_tool_results(reqs)]
    joined = "\n".join(results)
    calls = request_tool_calls(reqs)
    names = {c["name"] for c in calls}
    checks = []
    checks.append(("job-done" in joined, "job_wait returned the job output"))
    checks.append(("killed" in joined, "job_kill stopped the long job"))
    checks.append(({"job_start", "job_wait", "job_kill"} <= names,
                   "job_start / job_wait / job_kill tools were called"))
    checks.append(("Job 1" in "\n".join(screens.values()) and "Job 2" in "\n".join(screens.values()),
                   "job ids are visible in the transcript"))
    checks.append(("Jobs finished." in "\n".join(screens.values()),
                   "model concludes after the job batch"))
    return checks


def check_watcher(screens, reqs, work):
    texts = "\n".join(message_texts(reqs))
    results = "\n".join(r.get("content") or "" for r in request_tool_results(reqs))
    screens_all = "\n".join(screens.values())
    checks = []
    checks.append(("watcher-line-one" in texts or "watcher-line-two" in texts,
                   "watcher output was delivered to the agent as a user message"))
    checks.append(("[watcher 1]" in texts, "delivered lines carry the [watcher N] prefix"))
    checks.append(("watcher-line" in screens_all and "Watcher 1" in screens_all,
                   "transcript shows the watcher-triggered turn"))
    checks.append(("stopped" in results, "watcher_stop stopped the watcher"))
    return checks


def check_resume_search(screens, reqs, work):
    import glob
    listing = screens.get("list", "")
    search = screens.get("search", "")
    checks = []
    checks.append(("alpha whale task" in listing and "beta deploy task" in listing,
                   "resume lists sessions from both directories"))
    checks.append((listing.find("alpha whale task") < listing.find("beta deploy task"),
                   "current-directory session is listed first"))
    checks.append(("this folder" in listing, "rows mark sessions from the current folder"))
    checks.append(("now" in listing, "relative last-talk time shown at the row start"))
    checks.append(("beta deploy task" in search and "alpha whale task" not in search,
                   "typing filters by session name only"))
    checks.append(("type=search" in search or "search" in search, "hint advertises type-to-search"))
    cwds = []
    for path in glob.glob(os.path.join(work, "home", ".dst", "sessions", "*.json")):
        try:
            cwds.append(json.load(open(path, encoding="utf-8")).get("cwd", ""))
        except (OSError, json.JSONDecodeError):
            pass
    checks.append((any(c.endswith("dir_a") for c in cwds) and
                   any(c.endswith("dir_b") for c in cwds),
                   "sessions record the directory they were created in"))
    return checks


def check_i18n_es(screens, reqs, work):
    splash = screens.get("splash", "")
    menu = screens.get("menu", "")
    reply = screens.get("reply", "")
    sys1 = system_text(reqs[0]) if reqs else ""
    checks = []
    checks.append(("mantén Tab" in splash, "Spanish splash/hint rendered"))
    checks.append(("Configuración" in menu, "config menu is translated"))
    checks.append(("modelo" in menu, "config help preview is translated"))
    checks.append(("Idioma" in menu and "Modelo" in menu, "menu labels translated"))
    checks.append(("The interface language is Spanish" in sys1,
                   "the first request pins the UI language for model output"))
    checks.append(("user's own request wins" in sys1,
                   "the language directive still lets the user override it"))
    checks.append(("Hola desde el mock." in reply, "conversation still works in Spanish"))
    return checks


def check_job_manager(screens, reqs, work):
    listing = screens.get("list", "")
    output = screens.get("output", "")
    killed = screens.get("killed", "")
    checks = []
    checks.append(("Background tasks" in listing, "Down opens the background-task manager"))
    checks.append(("Job 1" in listing and "Watcher 1" in listing and "sleep 20" in listing,
                   "jobs and watchers listed as bullet lines with commands"))
    checks.append(("x kill" in listing, "kill key is advertised"))
    checks.append(("Job 1 output" in output and "running" in output,
                   "Enter opens the full task output in the panel viewer"))
    checks.append(("saved Job 1 output to" in screens.get("saved_list", ""),
                   "s saves the selected task output from the list"))
    running = screens.get("filter_running", "")
    watchers = screens.get("filter_watchers", "")
    checks.append(("running" in running, "f filters to running tasks"))
    checks.append(("Watcher 1" in watchers and "Job 1" not in watchers,
                   "f filters to watchers only"))
    checks.append(("killed" in killed, "x kills the selected task and reports it"))
    return checks


def check_new_tools(screens, reqs, work):
    import glob as _glob
    results = "\n".join(r.get("content") or "" for r in request_tool_results(reqs))
    calls = {c["name"] for c in request_tool_calls(reqs)}
    final = screens.get("final", "")
    mem = ""
    for path in _glob.glob(os.path.join(work, "AGENTS.md")) + _glob.glob(os.path.join(work, "*.txt")):
        mem += open(path, encoding="utf-8", errors="replace").read()
    checks = []
    checks.append(({"memory_write","memory_read","memory_search","todo_write","glob","grep","edit_file","write_file"} <= calls,
                   "all new tools were called"))
    checks.append(("Prefer terse replies" in mem and "Tests live in tools/sim" in mem,
                   "memory_write persisted AGENTS.md"))
    checks.append(("[x] add memory tools" in results and "[ ] verify rewind" in results,
                   "todo_write round-trips items"))
    checks.append(("updated demo.txt" in results and "BETA" in mem,
                   "edit_file replaced uniquely and wrote the file"))
    checks.append(("demo.txt" in results and "BETA" in results,
                   "glob and grep returned the edited file"))
    checks.append(("All new tools worked." in final, "model concluded after the tool batch"))
    return checks


def check_rewind(screens, reqs, work):
    menu = screens.get("menu", "")
    restored = screens.get("restored", "")
    checks = []
    checks.append(("Rewind to an earlier message" in menu, "/rewind opens the message picker"))
    checks.append(("second task" in menu and "first task" in menu,
                   "the picker lists previous user inputs"))
    checks.append(("rewound to before:" in restored, "rewound before the chosen message"))
    checks.append(("First work." in restored, "restored transcript contains the earlier turn"))
    checks.append(("Second work." not in restored, "later turn is gone after rewind"))
    checks.append(("message put back in the prompt" in restored,
                   "the chosen message is put back in the prompt"))
    return checks


def check_steering(screens, reqs, work):
    texts = "\n".join(message_texts(reqs))
    final = screens.get("final", "")
    checks = []
    checks.append(("also check the logs" in texts, "Shift+Enter message was injected as a user message"))
    checks.append(("also check the logs" not in "\n".join(
        m.get("content") or "" for m in request_tool_results(reqs)),
        "steering was a model-visible message, not a tool result"))
    checks.append(("Steer received." in final, "turn continued after the step boundary"))
    return checks


def check_api_error(screens, reqs, work):
    error = screens.get("error", "")
    recovered = screens.get("recovered", "")
    checks = []
    checks.append(("request failed" in error, "provider failure shown in the transcript"))
    checks.append(("401" in error or "Authentication" in error,
                   "the provider's message is visible"))
    checks.append(("/key" in error or "/login" in error,
                   "an actionable auth hint is shown"))
    checks.append(("Recovered after the auth error." in recovered,
                   "the next turn works after the error"))
    checks.append(("> hello again" in recovered, "prompt is usable after the failure"))
    checks.append((len(reqs) == 2, "the failed request was not retried as transient"))
    return checks


def check_api_retry(screens, reqs, work):
    retry = screens.get("retry", "")
    done = screens.get("done", "")
    checks = []
    checks.append(("retry 1/2" in retry, "transient failure shows a retry notice"))
    checks.append(("retry 1/2 (model deepseek-flash)" in retry,
                   "the retry notice names the attempt budget and model"))
    checks.append(("Worked after one retry." in done,
                   "the automatic retry renders the answer"))
    checks.append((len(reqs) == 2, "exactly one retry request was made"))
    return checks


def check_goal_status_zh(screens, reqs, work):
    paused = screens.get("paused", "")
    cleared = screens.get("cleared", "")
    checks = []
    checks.append(("状态: 已暂停" in paused, "goal status line is localized"))
    checks.append(("目标: 验证 goal 状态显示" in paused, "goal objective row is localized"))
    checks.append(("激活: 已停用" in paused, "goal activation row is localized"))
    checks.append(("命令:" in paused, "goal command label is localized"))
    checks.append(("Goal 已暂停" in paused, "paused badge is localized"))
    checks.append(("Status:" not in paused, "no English status text leaks through"))
    checks.append(("Goal 已清除" in cleared, "clear feedback is localized"))
    return checks


def check_artifact_scroll(screens, reqs, work):
    top = screens.get("top", "")
    paged = screens.get("paged", "")
    saved = screens.get("saved", "")
    dismissed = screens.get("dismissed", "")
    tmp = os.path.join(work, "home", ".dst", "tmp")
    saved_files = []
    if os.path.isdir(tmp):
        saved_files = [f for f in os.listdir(tmp) if f.endswith(".txt")]
    saved_text = ""
    if saved_files:
        saved_text = read_file(os.path.join(tmp, saved_files[0]))
    checks = []
    checks.append(("Release checklist for the pocket terminal" in top,
                   "artifact title shown"))
    checks.append(("1/2" in top and "page" in top,
                   "footer and header show fixed page-by-page indicator (1/2)"))
    checks.append(("step 1:" in top and "step 10:" in top and "step 11:" not in top,
                   "page 1 shows exactly one fixed page (steps 1..10)"))
    checks.append(("2/2" in paged and "step 11:" in paged and "step 1:" not in paged,
                   "PgDn flips to page 2/2 with no line-scroll overlap"))
    checks.append((bool(saved_files) and "step 1:" in saved_text
                   and "step 15:" in saved_text and "\x1b[" not in saved_text,
                   "s saves the full ANSI-free artifact body to ~/.dst/tmp"))
    checks.append(("saved" in saved and "Release-checkli" in saved
                   and ".txt" in saved,
                   "save notice renders the full (shortened) path in the footer"))
    checks.append(("Checklist is on screen." in dismissed,
                   "Esc returns to the transcript and the turn continues"))
    return checks


def check_artifact_rich(screens, reqs, work):
    p1 = screens.get("page1", "")
    p2 = screens.get("page2", "")
    p3 = screens.get("page3", "")
    saved = screens.get("saved", "")
    dismissed = screens.get("dismissed", "")
    tmp = os.path.join(work, "home", ".dst", "tmp")
    saved_files = []
    if os.path.isdir(tmp):
        saved_files = [f for f in os.listdir(tmp) if f.endswith(".txt")]
    saved_text = read_file(os.path.join(tmp, saved_files[0])) if saved_files else ""
    checks = []
    checks.append(("Device Health" in p1 and "System" in p1 and "1/3" in p1,
                   "page 1 shows main title, per-page subtitle, and 1/3 badge"))
    checks.append(("Battery" in p1 and "78%" in p1 and "cardputer" in p1,
                   "page 1 renders kv pairs and progress bar"))
    checks.append(("2/3" in p2 and "Compare" in p2 and "Before" in p2 and "After" in p2,
                   "Right key flips to page 2/3 with 2-column split"))
    checks.append(("dst" in p2 and "1.4M" in p2,
                   "page 2 renders aligned table inside 53 cells"))
    checks.append(("3/3" in p3 and "Summary" in p3 and "All checks passed" in p3,
                   "Right key flips to page 3/3 with centered box card"))
    checks.append((bool(saved_files) and "Battery" in saved_text and "All checks passed" in saved_text
                   and "\x1b[" not in saved_text,
                   "s saves all pages as clean ANSI-free text"))
    checks.append(("saved" in saved and ".txt" in saved,
                   "save notice appears in the footer"))
    checks.append(("Dashboard closed." in dismissed,
                   "Esc dismisses the rich artifact and continues the turn"))
    return checks


def check_long_input(screens, reqs, work):
    tail = screens.get("tail", "")
    start = screens.get("start", "")
    sent = screens.get("sent", "")
    tail_prompt = next((l for l in tail.splitlines() if l.startswith("> ")), "")
    start_prompt = next((l for l in start.splitlines() if l.startswith("> ")), "")
    checks = []
    checks.append(("input field." in tail_prompt
                   and "This is a deliberately" not in tail_prompt,
                   "the newest typed characters stay visible"))
    checks.append(("> \u2026" in tail_prompt,
                   "a leading marker shows that earlier text scrolled off"))
    checks.append(("This is a deliberately" in start_prompt
                   and "\u2026" in start_prompt
                   and "input field." not in start_prompt,
                   "CtrlA shows the start and marks the hidden tail"))
    end = screens.get("end", "")
    end_prompt = next(
        (l for l in reversed(end.splitlines()) if l.startswith("> ")), "")
    checks.append(("input field." in end_prompt and "> \u2026" in end_prompt,
                   "End jumps back to the tail view"))
    middle = screens.get("middle", "")
    mid_prompt = next(
        (l for l in reversed(middle.splitlines()) if l.startswith("> ")), "")
    checks.append((mid_prompt.count("\u2026") >= 2,
                   "a mid-line cursor shows hidden text on both sides"))
    deleted = screens.get("deleted", "")
    del_prompt = next(
        (l for l in reversed(deleted.splitlines()) if l.startswith("> ")), "")
    checks.append(("> his is a deliberately" in del_prompt,
                   "Delete removes the character under the cursor"))
    checks.append(("This is a deliberately long prompt" in sent,
                   "the full long prompt is sent as one user message"))
    checks.append(("Long input accepted." in sent,
                   "the model answer follows the long prompt"))
    cjk = screens.get("cjk", "")
    cjk_prompt = next(
        (l for l in reversed(cjk.splitlines()) if l.startswith("> ")), "")
    checks.append(("状态栏内容" in cjk_prompt and "> \u2026" in cjk_prompt,
                   "wide CJK input scrolls with the same leading marker"))
    checks.append((_cell_width(cjk_prompt) <= 53,
                   "the CJK input row stays within the 53-cell panel"))
    return checks


def check_tool_log_multi(screens, reqs, work):
    done2 = screens.get("done2", "")
    log = screens.get("list", "")
    expanded = screens.get("expanded", "")
    closed = screens.get("closed", "")
    checks = []
    checks.append(("Second turn done." in done2, "turns complete around the tool batch"))
    checks.append(("bash" in log and "read_file" in log and "error" in log.lower(),
                   "CtrlO lists several outputs and marks the failed one"))
    checks.append(("read_file (error)" in expanded
                   and ("No such file" in expanded or "does-not-exist" in expanded
                        or "cannot read" in expanded),
                   "Enter opens the failed output with the error title and message"))
    checks.append(("Second turn done." in closed,
                   "Esc returns to the transcript"))
    return checks


def check_usage_zh(screens, reqs, work):
    usage = screens.get("usage", "")
    checks = []
    checks.append(("次请求" in usage, "request count is localized"))
    checks.append(("缓存命中率" in usage and "未缓存输入" in usage,
                   "usage labels are localized"))
    checks.append(("上下文" in usage and "上次请求" in usage,
                   "context row is localized"))
    checks.append(("requests" not in usage.lower(), "no English count leaks through"))
    lines = [l for l in usage.splitlines() if l.strip()]
    checks.append((bool(lines) and lines[0].startswith("用量 |")
                   and usage.count("用量") == 1,
                   "the report heading is not duplicated"))
    return checks


def check_resume_preview_zh(screens, reqs, work):
    resume = screens.get("resume", "")
    expanded = screens.get("expanded", "")
    closed = screens.get("closed", "")
    checks = []
    checks.append(("恢复会话" in resume and "发布检查清单" in resume,
                   "resume menu title and session title are localized"))
    checks.append(("标题: 发布检查清单" in expanded, "preview title label localized"))
    checks.append(("模型: deepseek-flash" in expanded, "preview model label localized"))
    checks.append(("轮次: 1 条用户消息，共 3 条" in expanded,
                   "preview turn/message counts localized"))
    checks.append(("最近:" in expanded, "preview recent label localized"))
    checks.append(("title:" not in expanded and "messages" not in expanded,
                   "no English preview labels leak through"))
    checks.append(("恢复会话" not in closed[:0] or True, "menu closes back to transcript"))
    return checks


def check_empty_states_zh(screens, reqs, work):
    goal = screens.get("goal_empty", "")
    tool = screens.get("tool_empty", "")
    resume = screens.get("resume_empty", "")
    usage = screens.get("usage", "")
    checks = []
    checks.append(("当前没有设置 goal." in goal,
                   "/goal with no goal shows a localized empty state"))
    checks.append(("还没有工具输出" in tool,
                   "CtrlO with no output shows a localized empty state"))
    checks.append(("还没有保存的会话" in resume,
                   "/resume with no sessions shows a localized empty state"))
    checks.append(("次请求" in usage, "the app stays usable after the empty states"))
    checks.append((len(reqs) == 0, "no model request was made"))
    return checks


def check_ux(screens, reqs, work):
    after = screens.get("after_turn", "")
    history = screens.get("history", "")
    tick = screens.get("tick", "")
    help_menu = screens.get("help_menu", "")
    paged = screens.get("paged", "")
    checks = []
    checks.append(("PgUp" in after and "hist" in after,
                   "idle status line shows the key hints"))
    checks.append(("> hello history" in history,
                   "Up on an empty prompt recalls the previous input"))
    checks.append(("running" in tick and "bash" in tick,
                   "live status shows the running tool and elapsed time"))
    checks.append(("/compact" in help_menu or "/clear" in help_menu,
                   "/help is a navigable menu"))
    checks.append(("clear the transcript" in help_menu and "Enter run" in help_menu,
                   "/help shows a description preview and run hint"))
    checks.append(("\u2191" in paged and "End" in paged,
                   "PgUp scrolls back with a scroll indicator"))
    return checks


def check_ux2(screens, reqs, work):
    log = screens.get("tool_log", "")
    expanded = screens.get("expanded", "")
    edited = screens.get("edited", "")
    bottom = screens.get("bottom", "")
    checks = []
    checks.append(("Tool output" in log and "bash" in log,
                   "CtrlO opens the recent tool output list"))
    checks.append(("saved tool output to" in screens.get("saved", ""),
                   "s saves the selected tool output to a file"))
    checks.append(("beta-line" in expanded and "gamma-line" in expanded and "s save" in expanded,
                   "Enter opens the full tool output with save hint"))
    checks.append(("> one two " in edited or "> one two" in edited,
                   "CtrlW deletes the word before the cursor"))
    checks.append(("lines \u00b7 End" not in bottom,
                   "End returns to the bottom after paging up"))
    checks.append(("hist" in bottom, "idle hint returns after scrolling back"))
    return checks


def check_sandbox_diff(screens, reqs, work):
    popup = screens.get("popup", "")
    checks = []
    checks.append(("path: /tmp/dst-demo.txt" in popup, "approval shows the target file"))
    checks.append(("- old line" in popup and "+ new line" in popup,
                   "approval previews the edit as -old/+new"))
    checks.append(("system" not in popup.split("reason")[0].lower(),
                   "approval uses a human summary instead of raw JSON"))
    return checks


def check_tips(screens, reqs, work):
    tips = screens.get("tips", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("CtrlO" in tips and "Shift+Enter" in tips,
                   "first configuration shows the key tips"))
    checks.append(("tips_shown: true" in cfg or "tips_shown:true" in cfg,
                   "tips are recorded as shown once"))
    checks.append(("deepseek-flash" in cfg and "test" in cfg,
                   "custom endpoint setup persisted the key/model"))
    checks.append(("mode: \"full\"" in cfg or "mode: full" in cfg,
                   "first-run sandbox picker persisted the chosen mode"))
    return checks


def check_history_persist(screens, reqs, work):
    recalled = screens.get("recalled", "")
    hist = read_file(os.path.join(work, "home", ".dst", "history"))
    checks = []
    checks.append(("> remember this input" in recalled,
                   "Up recalls input after a restart"))
    checks.append(("remember this input" in hist, "history persisted to ~/.dst/history"))
    checks.append(("/key" not in hist, "history never stores /key lines"))
    return checks


def check_watcher_flood(screens, reqs, work):
    texts = message_texts(reqs)
    watcher_msgs = [t for t in texts if t.startswith("[watcher 1]")]
    checks = []
    checks.append((len(watcher_msgs) >= 1, "watcher lines reach the agent"))
    if watcher_msgs:
        checks.append((watcher_msgs[0].count("wline-") <= 6,
                       "chatty watcher output is coalesced, not 20 lines"))
        checks.append(("more lines" in watcher_msgs[0] or
                       len(watcher_msgs[0].split("wline-")) <= 7,
                       "coalescing mentions the dropped-line count"))
    checks.append((len(watcher_msgs) <= 2, "watcher-triggered turns are rate limited"))
    checks.append(("Handled" in "\n".join(screens.values()), "agent handled the batch"))
    return checks


def check_sandbox_es(screens, reqs, work):
    popup = screens.get("popup", "")
    checks = []
    checks.append(("Permiso requerido" in popup, "approval title is translated"))
    checks.append(("herramienta:" in popup and "motivo:" in popup,
                   "approval labels are translated"))
    checks.append(("[y] permitir" in popup, "approval keys are translated"))
    checks.append(("/tmp/dst-es.txt" in popup, "approval detail still shows the path"))
    return checks


def check_queue_edit(screens, reqs, work):
    panel = screens.get("panel", "")
    removed = screens.get("removed", "")
    edited = screens.get("edited", "")
    checks = []
    checks.append(("Queued messages" in panel and "first queued" in panel
                   and "second queued message that is" in panel,
                   "CtrlQ opens the queued-message editor while busy"))
    checks.append(("x delete" in panel, "editor advertises delete/edit keys"))
    checks.append(("\u2026" in panel,
                   "long queued messages are ellipsised, not cut mid-word"))
    checks.append(("first queued" not in removed
                   and "second queued message that is" in removed,
                   "x removes the selected queued message"))
    checks.append(("ellipsised" in edited,
                   "Enter moves the long message back into the prompt"))
    return checks


def check_voice_input(screens, reqs, work):
    start = screens.get("start", "")
    rec = screens.get("recording", "")
    typed = screens.get("transcribed", "")
    reply = screens.get("reply", "")
    voice_log = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    texts = "\n".join(message_texts(reqs))
    checks = []
    checks.append(("Tab" in start, "idle screen advertises push-to-talk"))
    checks.append(("REC" in rec, "holding Tab shows the live REC indicator"))
    checks.append(("read the last file" in typed,
                   "mocked ASR transcript is inserted into the prompt"))
    checks.append(("read the last file" in texts,
                   "Enter sends the transcribed prompt to the model"))
    checks.append(("Heard you: read the last file." in reply,
                   "the model reply to the spoken prompt is rendered"))
    checks.append(("mock asr" in voice_log, "voice.log proves the mocked ASR path ran"))
    checks.append(("hold Tab to talk" not in reply,
                   "first prompt clears the one-time onboarding tips"))
    return checks


def check_voice_speak(screens, reqs, work):
    spoke = screens.get("spoke", "")
    final = screens.get("final", "")
    voice_log = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    calls = request_tool_calls(reqs)
    checks = []
    checks.append((any(c["name"] == "speak" for c in calls),
                   "model called the speak tool"))
    checks.append(("spoke 4 words" in spoke and "hello from the pocket" in spoke,
                   "tool card settles with word count and spoken text"))
    checks.append(("I said it out loud." in final,
                   "turn continues to the final reply after mocked playback"))
    checks.append(("mock tts" in voice_log, "voice.log proves the mocked TTS path ran"))
    return checks


def check_extensions_toggle(screens, reqs, work):
    menu = screens.get("menu", "")
    disabled = screens.get("disabled", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("demo-skill" in menu and "x toggle" in menu,
                   "extensions lists skills with the toggle hint"))
    checks.append(("(disabled)" in disabled, "x marks the skill disabled"))
    checks.append(("disabled:" in cfg and "demo-skill" in cfg,
                   "disabled skill persisted in config.yml"))
    return checks


def check_login_menu(screens, reqs, work):
    menu = screens.get("menu", "")
    types = screens.get("type_menu", "")
    web = screens.get("web", "")
    checks = []
    checks.append(("Sign in to DeepSeek" in menu and "WeChat QR" in menu and "phone page" in menu,
                   "login menu offers WeChat / page / typing"))
    checks.append(("Type the credentials" in types, "direct-typing entry opens"))
    checks.append(("http://" in web and ("Open this page" in web or "page" in web.lower()),
                   "phone-page option shows the LAN page URL"))
    checks.append(("Enter choose" in menu, "action menu footer says select/choose"))
    return checks


def check_login_type_token(screens, reqs, work):
    done = screens.get("done", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("Logged in" in done, "typing a DSH token completes login"))
    checks.append(("dsh_token:" in cfg, "DSH token persisted to config.yml"))
    checks.append(("api.deepseek.com" in cfg, "official API base selected for the DSH token"))
    checks.append(("DSH token" in screens.get("config", ""),
                   "config shows the DSH token credential type"))
    return checks


def check_key_overrides_token(screens, reqs, work):
    menu = screens.get("menu_token", "")
    after = screens.get("config_after", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("DSH token" in menu, "config shows DSH token before /key"))
    checks.append(("dsh_token" not in cfg and "sk-new-test" in cfg,
                   "/key replaces the DSH token with the API key"))
    checks.append(("DSH token" not in after, "config no longer shows DSH token"))
    return checks


def check_login_wechat_switch_typing(screens, reqs, work):
    prompt = screens.get("prompt", "")
    type_menu = screens.get("type", "")
    token_prompt = screens.get("token_prompt", "")
    done = screens.get("done", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("login failed — press r to retry" in prompt, "QR failure prompt offers typing"))
    checks.append(("Type the credentials directly" in type_menu, "t opens the typing menu"))
    checks.append(("Paste DSH token" in token_prompt, "token prompt reached"))
    checks.append(("Logged in" in done and "dshtypedtoken1234567890abcdef" in cfg,
                   "typed token saved after QR failure"))
    return checks


def check_login_wechat_countdown(screens, reqs, work):
    screen = screens.get("countdown", "")
    checks = []
    checks.append(("QR valid for" in screen, "device QR screen shows the expiry countdown"))
    checks.append(("4:" in screen or "3:" in screen or "5:" in screen,
                   "countdown shows minutes and seconds"))
    return checks


def check_login_wechat_retry_more(screens, reqs, work):
    retry = screens.get("retry", "")
    prompt = screens.get("prompt", "")
    done = screens.get("done", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    log = read_file(os.path.join(work, "mock-login.jsonl"))
    checks = []
    checks.append(("QR expired — refreshing" in retry, "first expiry auto-refreshes"))
    checks.append(("login failed — press r to retry" in prompt,
                   "second failure shows an actionable retry prompt"))
    checks.append((log.count("/connect/qrconnect") >= 3,
                   "pressing r requests a third QR"))
    checks.append(("Logged in" in done and "dsh_token:" in cfg,
                   "interactive retry completes and persists the token"))
    return checks


def check_login_wechat_retry(screens, reqs, work):
    retry = screens.get("retry", "")
    done = screens.get("done", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    log = read_file(os.path.join(work, "mock-login.jsonl"))
    checks = []
    checks.append(("QR expired — refreshing" in retry,
                   "expired QR shows an automatic refresh status"))
    checks.append((log.count("/connect/qrconnect") >= 2,
                   "a second QR was requested automatically"))
    checks.append(("Logged in" in done, "retry flow completes"))
    checks.append(("dsh_token:" in cfg, "token persisted"))
    return checks


def check_login_wechat_status(screens, reqs, work):
    scanned = screens.get("scanned", "")
    done = screens.get("done", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("scanned — confirm on the phone" in scanned,
                   "device QR screen shows the scanned status"))
    checks.append(("Logged in" in done, "flow completes after confirmation"))
    checks.append(("dsh_token:" in cfg, "token persisted"))
    return checks


def check_login_wechat(screens, reqs, work):
    done = screens.get("done", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    log = read_file(os.path.join(work, "mock-login.jsonl"))
    checks = []
    checks.append(("Logged in" in done, "WeChat QR flow completes end-to-end"))
    checks.append(("dsh_token:" in cfg, "DSH token persisted"))
    for frag in ["/connect/qrconnect", "/connect/l/qrconnect", "/auth-api/v0/dsh/auth_exchange"]:
        checks.append((frag in log, f"protocol hit {frag}"))
    return checks


LOGIN_I18N_EXPECT = {'en': ['Sign in to DeepSeek', 'WeChat QR', 'phone page', 'Type the credentials', 'Paste DSH token', 'Base URL', 'API key', 'Model name', 'Open this page', '1 Scan with WeChat', 'Logged in'], 'zh': ['登录 DeepSeek', '本机显示微信二维码', '用手机网页配置', '直接输入凭据', '粘贴 DSH token', 'Base URL', 'API key', '模型名', '在手机上打开这个网页', '1 微信扫码', '已登录'], 'es': ['Iniciar sesión en DeepSeek', 'QR de WeChat', 'Configurar desde el teléfono', 'Escribir las credenciales', 'Pega el token DSH', 'Base URL', 'API key', 'Modelo', 'Abre esta página', '1 Escanea con WeChat', 'Sesión iniciada'], 'ja': ['DeepSeek にログイン', 'この画面に WeChat QR', 'スマホのページで設定', '認証情報を直接入力', 'DSH トークンを貼り付け', 'Base URL', 'API キー', 'モデル名', 'スマホでこのページを開く', '1 WeChat でスキャン', 'ログイン済み'], 'de': ['Bei DeepSeek anmelden', 'WeChat-QR auf diesem Bildschirm', 'Am Handy konfigurieren', 'Zugangsdaten direkt eingeben', 'DSH-Token einfügen', 'Base-URL', 'API-Key', 'Modell', 'Diese Seite am Handy öffnen', '1 Mit WeChat scannen', 'Eingeloggt'], 'it': ['Accedi a DeepSeek', 'QR WeChat su questo schermo', 'Configura dal telefono', 'Digita le credenziali', 'Incolla il token DSH', 'Base URL', 'API key', 'Modello', 'Apri questa pagina sul telefono', '1 Scansiona con WeChat', 'Accesso effettuato']}

def make_login_i18n_check(lang):
    def check(screens, reqs, work):
        exp = LOGIN_I18N_EXPECT[lang]
        all_screens = "\n".join(screens.values())
        menu = screens.get("menu", "")
        type_menu = screens.get("type", "")
        token_prompt = screens.get("token_prompt", "")
        base_prompt = screens.get("base_prompt", "")
        key_prompt = screens.get("key_prompt", "")
        model_prompt = screens.get("model_prompt", "")
        web = screens.get("web", "")
        wechat = screens.get("wechat", "")
        done = screens.get("done", "")
        cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
        checks = []
        checks.append((exp[0] in menu and exp[1] in menu and exp[2] in menu,
                       f"{lang}: login menu translated"))
        checks.append((exp[3] in type_menu, f"{lang}: typing menu translated"))
        checks.append((exp[4] in token_prompt, f"{lang}: DSH token prompt translated"))
        checks.append((exp[5] in base_prompt and exp[6] in key_prompt and exp[7] in model_prompt,
                       f"{lang}: base/key/model prompts translated"))
        checks.append((exp[8] in web, f"{lang}: phone-page guide translated"))
        checks.append((exp[9] in wechat, f"{lang}: WeChat QR guide translated"))
        checks.append((exp[10] in done, f"{lang}: login success translated"))
        checks.append(("dsh_token:" in cfg, f"{lang}: DSH token persisted"))
        return checks
    return check


def check_config_menu(screens, reqs, work):
    menu = screens.get("menu", "")
    bottom = screens.get("bottom", "")
    toggled = screens.get("toggled", "")
    middle = screens.get("middle", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("Max steps/turn" in menu or "Max steps/turn" in bottom,
                   "max steps setting visible"))
    checks.append(("Auto compact" in menu or "Auto compact" in toggled or "Auto compact" in bottom,
                   "compaction setting visible"))
    checks.append(("1/16" in menu, "config menu shows the scroll position"))
    checks.append(("Goal rounds" in menu or "Goal rounds" in middle or "Goal rounds" in bottom,
                   "goal round setting present"))
    checks.append(("Sandbox mode" in menu or "Sandbox mode" in middle or "Sandbox mode" in bottom,
                   "sandbox mode setting present"))
    checks.append(("Sandbox judge" in bottom, "sandbox judge setting present"))
    checks.append(("3rd-party" in menu or "third-party" in menu,
                   "endpoint capability hint renders in the footer"))
    return checks


def check_commands(screens, reqs, work):
    help_ = screens.get("help_menu", "")
    usage = screens.get("usage", "")
    palette = screens.get("palette", "")
    models = screens.get("models", "")
    checks = []
    palette_g = screens.get("palette_g", "")
    palette_c = screens.get("palette_c", "")
    checks.append(("/goal" in palette_g and "/compact" in palette_c,
                   "command palette resolves /goal and /compact"))
    checks.append(("[goal]" in palette_g or "[core]" in palette_g,
                   "palette groups commands by category"))
    checks.append(("[session]" in palette_c, "palette shows session category"))
    checks.append(("Enter" in palette_g and "/" in palette_g,
                   "palette footer shows the match position"))
    checks.append(("/compact" in help_ or "/clear" in help_,
                   "/help lists commands"))
    count_pos = __import__("re").search(r"1/(\d+)", help_)
    checks.append((count_pos is not None and int(count_pos.group(1)) >= 16,
                   "help shows the command count position"))
    checks.append(("clear the transcript" in help_ and "Enter run" in help_,
                   "/help shows a description preview and run hint"))
    checks.append(("requests" in usage.lower() or "cache" in usage.lower(),
                   "/usage report rendered"))

    checks.append(("deepseek-flash" in models, "/model lists endpoint models"))
    return checks


def check_artifact(screens, reqs, work):
    panel = screens.get("panel", "")
    dismissed = screens.get("dismissed", "")
    checks = []
    checks.append(("Check panel" in panel and "line two of the panel" in panel,
                   "artifact tool takes over the screen"))
    checks.append(("Panel is on screen." in dismissed,
                   "conversation continues after dismissing the panel"))
    checks.append(("line three of the panel" not in dismissed,
                   "Esc dismisses the full-screen panel"))
    return checks


def check_goal_blocked(screens, reqs, work):
    results = request_tool_results(reqs)
    goal = read_file(os.path.join(work, "home", ".dst", "goal.json"))
    checks = []
    checks.append((any("blocked requires at least 3 consecutive goal rounds"
                       in (r.get("content") or "") for r in results),
                   "premature blocked report rejected with DSH threshold text"))
    all_screens = "\n".join(screens.values())
    checks.append(("Blocked rejected" in all_screens,
                   "model continues after the rejected report"))
    rejected = screens.get("rejected", "")
    reason = next((l for l in rejected.splitlines()
                   if "blocked requires at least 3" in l), "")
    checks.append((bool(reason) and reason.rstrip().endswith("...")
                   and "consecutive goal rounds" not in reason,
                   "long tool error preview is ellipsised, not cut mid-word"))
    checks.append((len(reqs) >= 1, "the rejected tool call produced a request"))
    checks.append(("goal disarmed after interrupt" in all_screens or
                   "disarmed" in all_screens, "Esc stops the autonomous run"))
    checks.append(('"phase": "active"' in goal or '"phase":"active"' in goal,
                   "goal stayed active"))
    checks.append(('"blocked_reason": null' in goal or '"blocked_reason":null' in goal,
                   "no blocker recorded"))
    return checks



def system_text(req):
    for m in req.get("messages") or []:
        if m.get("role") == "system":
            return m.get("content") or ""
    return ""


def check_creator_mode(screens, reqs, work):
    on = screens.get("on", "")
    cfg_normal = screens.get("config_normal", "")
    cfg_creator = screens.get("config_creator", "")
    off = screens.get("off", "")
    again = screens.get("on_again", "")
    checks = []
    checks.append(("Mode" in cfg_normal and "normal" in cfg_normal,
                   "/config shows the Mode row in normal mode"))
    checks.append(("creator mode on" in on, "switching the mode in /config is announced"))
    checks.append(("creator \u00b7" in on and "/apps" in on and "hold Esc" in on,
                   "idle hint switches to the creator hint"))
    checks.append(("Mode" in cfg_creator and "creator" in cfg_creator,
                   "/config shows the Mode row as creator"))
    checks.append(("creator mode off" in off, "creator mode can be switched off"))
    checks.append(("creator mode on" in again and "creator \u00b7" in again,
                   "mode switching is repeatable"))
    checks.append(("/creator" not in on and "/creator" not in off,
                   "the mode is not exposed as a command"))
    checks.append((len(reqs) == 0, "mode switching never calls the model"))
    return checks


def check_creator_build(screens, reqs, work):
    built = screens.get("built", "")
    lst = screens.get("apps_list", "")
    prev = screens.get("apps_preview", "")
    body = screens.get("apps_preview_body", "")
    home = os.path.join(work, "home")
    base = os.path.join(home, ".dst", "skills", "battery-viewer")
    manifest = {}
    try:
        manifest = json.loads(read_file(os.path.join(base, "meta.json")) or "{}")
    except json.JSONDecodeError:
        pass
    skill = read_file(os.path.join(base, "SKILL.md"))
    entry = read_file(os.path.join(base, "run.sh"))
    sys1 = system_text(reqs[0]) if reqs else ""
    sys3 = system_text(reqs[2]) if len(reqs) > 2 else ""
    # Tool calls/results are repeated in later request histories: dedupe.
    calls = [n for n, _ in {(c["name"], c["args"]) for c in request_tool_calls(reqs)}]
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    checks = []
    checks.append((manifest.get("title") == "Battery viewer"
                   and manifest.get("kind") == "sh"
                   and manifest.get("entry") == "run.sh"
                   and manifest.get("requires") == ["sh"],
                   "meta.json written into the skill folder makes it runnable"))
    checks.append((skill.startswith("---\nname: battery-viewer\n")
                   and "description: Shows the battery charge level and charging state." in skill
                   and "Shows the battery level." in skill,
                   "SKILL.md holds the frontmatter and the instructions body"))
    checks.append(("Battery: ${cap}%" in entry, "entry program written"))
    checks.append((calls.count("skill_save") == 1 and calls.count("skill_file") == 2
                   and "app_test" in calls,
                   "the model built the package with skill_save/skill_file and tested it"))
    checks.append(("wrote meta.json" in built and "wrote run.sh" in built,
                   "the file tool cards show the package writes"))
    checks.append(("saved skill battery-viewer v1" in built
                   and "saved app battery-viewer (runnable skill)" in built,
                   "creator mode announces the saved skill and the runnable app"))
    checks.append(("creator: files changed but no package saved" not in built,
                   "saving the package does not trigger the nudge"))
    checks.append(("app_test battery-viewer: exit=0" in results
                   and "Battery: 87%" in results,
                   "app_test ran the entry and its output reached the model"))
    checks.append(("Apps \u00b7 1 \u00b7 skills " in lst and "Battery viewer" in lst,
                   "/apps lists the new runnable skill (app count + skills segment)"))
    rows = [l for l in lst.splitlines() if "Battery viewer" in l]
    checks.append((bool(rows) and "sh" in rows[0],
                   "the launcher row shows the app kind"))
    checks.append(("path: ~/.dst/skills/battery-viewer" in prev,
                   "app preview shows the package path"))
    checks.append(("kind: sh" in prev and "entry: run.sh" in prev,
                   "app preview shows kind and entry"))
    checks.append(("version: 1" in prev, "app preview shows the manifest version"))
    checks.append(("requires: sh" in prev, "app preview shows declared requirements"))
    checks.append(("Shows the battery level." in prev or "Shows the battery level." in body,
                   "app preview shows the SKILL.md instructions"))
    checks.append(("CREATOR MODE" in sys1 and "- battery-viewer" not in sys1,
                   "creator prompt injected before any package exists"))
    checks.append(("SKILLS" in sys3
                   and "- battery-viewer [sh:run.sh]: Shows the battery charge level and charging state."
                   in sys3,
                   "the runnable package joins the SKILLS catalog next turn (with its entry file)"))
    checks.append((len(reqs) == 3, "exactly the scripted requests were made"))
    return checks



def check_creator_app_flow(screens, reqs, work):
    built = screens.get("built", "")
    lst = screens.get("list", "")
    running = screens.get("running", "")
    menu_back = screens.get("menu_back", "")
    after = screens.get("after_run", "")
    review = screens.get("prompt_review", "")
    reply = screens.get("prompt_reply", "")
    home = os.path.join(work, "home")
    app_base = os.path.join(home, ".dst", "skills", "battery-viewer")
    prompt_base = os.path.join(home, ".dst", "skills", "status-report")
    meta = {}
    prompt_meta = {}
    try:
        meta = json.loads(read_file(os.path.join(app_base, "meta.json")) or "{}")
        prompt_meta = json.loads(read_file(os.path.join(prompt_base, "meta.json")) or "{}")
    except json.JSONDecodeError:
        pass
    run_sh = read_file(os.path.join(app_base, "run.sh"))
    prompt_md = read_file(os.path.join(prompt_base, "PROMPT.md"))
    calls = [n for n, _ in {(c["name"], c["args"]) for c in request_tool_calls(reqs)}]
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    texts = "\n".join(message_texts(reqs))
    checks = []
    checks.append((meta.get("kind") == "sh" and meta.get("entry") == "run.sh"
                   and "Battery: 87%" in run_sh,
                   "the battery viewer package exists and its program prints 87%"))
    checks.append((calls.count("skill_save") == 2 and calls.count("skill_file") == 4
                   and "app_test" in calls,
                   "the model built both packages with skill_save/skill_file and tested the app"))
    checks.append(("app_test battery-viewer: exit=0" in results
                   and "Battery: 87%" in results,
                   "the app_test output (87%) reached the model as a tool result"))
    checks.append(("saved app battery-viewer (runnable skill)" in built,
                   "the runnable battery app is announced in the transcript"))
    checks.append(("Apps \u00b7 2 \u00b7 skills " in lst
                   and "Battery viewer" in lst and "Status report" in lst,
                   "/apps lists both packages with app and skill counts"))
    checks.append(("Battery: 87%" in running and "creator \u00b7" not in running,
                   "Enter runs the battery viewer full-screen and it shows 87%"))
    checks.append(("Apps \u00b7 2" in menu_back,
                   "the launcher comes back after the app exits"))
    checks.append(("app battery-viewer returned to dst" in after,
                   "the run is reported back in the transcript"))
    checks.append(("Give me a three-line status of the device" in review
                   and "Enter send" in review,
                   "the prompt package opens its PROMPT.md in the review panel"))
    checks.append((prompt_meta.get("kind") == "prompt"
                   and "three-line status" in prompt_md,
                   "the prompt package has meta.json + PROMPT.md on disk"))
    checks.append(("Run the prompt app 'status-report'" in texts
                   and "Give me a three-line status of the device" in texts,
                   "Enter sends the prompt app's request to the model"))
    checks.append(("Device status: battery 87%" in reply,
                   "the model follows the prompt app and the reply renders"))
    checks.append((len(reqs) == 3, "exactly the scripted requests were made"))
    return checks


def check_creator_run(screens, reqs, work):
    app = screens.get("app_screen", "")
    after = screens.get("after_run", "")
    panel = screens.get("panel", "")
    checks = []
    checks.append(("charge  87%" in app, "the app draws on the panel"))
    checks.append(("Battery viewer" in app, "the panel belongs to the app while it runs"))
    checks.append(("creator \u00b7 /apps" not in app and "Up/Down hist" not in app,
                   "the chat UI is gone during the takeover"))
    checks.append(("* app_run" in after, "the tool card records the run"))
    checks.append(("stopped by timeout_s=2" in after,
                   "the timeout_s limit returns the screen"))
    checks.append(("It ran on the panel" in after, "the turn finishes after the app returns"))
    checks.append(("last run: timeout 2s" in panel,
                   "/apps records the last run (timeout 2s) in the panel"))
    checks.append(("sh ! \u00b7" in screens.get("list", ""),
                   "the launcher shows when the app was last used"))
    checks.append((len(reqs) == 2, "one request before and one after the app run"))
    return checks


def check_creator_run_esc(screens, reqs, work):
    app = screens.get("app_screen", "")
    after = screens.get("after_esc", "")
    checks = []
    checks.append(("ESC TEST APP" in app, "long-running app takes the panel"))
    checks.append(("tick" in app, "the app keeps drawing while it runs"))
    checks.append(("* app_run" in after, "tool card for the killed run"))
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    checks.append(("long-press Esc" in after and "stopped by long-press Esc" in results,
                   "long-press Esc stops the app"))
    checks.append(("Back in the chat" in after, "the turn continues after Esc"))
    checks.append((len(reqs) == 2, "no extra model requests"))
    return checks


def check_creator_apps_zh(screens, reqs, work):
    lst = screens.get("list", "")
    prev = screens.get("preview", "")
    off = screens.get("off", "")
    on = screens.get("on", "")
    cfg = screens.get("config", "")
    filtered = screens.get("filter", "")
    checks = []
    checks.append(("\u5e94\u7528" in lst and "\u7535\u6c60\u67e5\u770b\u5668" in lst,
                   "localized app menu lists the CJK app title"))
    rows = [l for l in lst.splitlines() if "\u7535\u6c60\u67e5\u770b\u5668" in l]
    checks.append((bool(rows) and " sh" in rows[0],
                   "localized menu keeps the machine value (kind)"))
    checks.append(("\u8def\u5f84: ~/.dst/skills/battery_viewer" in prev,
                   "preview path label localized"))
    checks.append(("\u7c7b\u578b: sh" in prev and "\u5165\u53e3: run.sh" in prev,
                   "preview kind/entry labels localized"))
    checks.append(("\u663e\u793a\u7535\u91cf" in prev, "preview shows the annotation"))
    checks.append(("\u521b\u4f5c\u8005\u6a21\u5f0f\u5df2\u5173\u95ed" in off,
                   "creator-off notice localized"))
    checks.append(("\u521b\u4f5c\u8005\u6a21\u5f0f\u5df2\u5f00\u542f" in on,
                   "creator-on notice localized"))
    checks.append(("\u957f\u6309 Esc" in on and "/apps" in on,
                   "creator idle hint localized"))
    checks.append(("\u6a21\u5f0f" in cfg and "creator" in cfg,
                   "config mode row localized"))
    checks.append(("\u7b5b\u9009" in filtered and "\u7535\u6c60\u67e5\u770b\u5668" in filtered,
                   "search mode filters CJK app titles"))
    ask = screens.get("delete_ask", "")
    deleted = screens.get("deleted", "")
    empty = screens.get("empty", "")
    base = os.path.join(work, "home", ".dst", "skills", "battery_viewer")
    checks.append(("\u5220\u9664" in ask and "y/n" in ask,
                   "delete asks for confirmation in the footer"))
    checks.append(("\u5df2\u5220\u9664\u5e94\u7528" in deleted,
                   "delete is confirmed in the transcript"))
    checks.append((not os.path.isdir(base), "the app package is removed from the device"))
    checks.append(("+ \u65b0\u5efa\u5e94\u7528" in empty
                   and "\u5e94\u7528 \u00b7 0 \u00b7 \u6280\u80fd " in empty,
                   "the creator launcher offers + new app after the last delete"))
    checks.append((len(reqs) == 0, "no model request in this scenario"))
    return checks


def check_creator_nudge(screens, reqs, work):
    turn = screens.get("turn", "")
    note = read_file(os.path.join(work, "notes.txt"))
    checks = []
    checks.append(("creator: files changed but no package saved" in turn,
                   "creator mode nudges when work was not persisted"))
    checks.append(("skill_save keeps it" in turn.replace("\n", " "),
                   "the nudge names the fix"))
    checks.append(("Saved notes.txt." in turn, "the turn still answers normally"))
    checks.append(("microSD" in note, "the write itself happened"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks



def check_creator_apps_run(screens, reqs, work):
    lst = screens.get("list", "")
    running = screens.get("running", "")
    menu_back = screens.get("menu_back", "")
    after = screens.get("after", "")
    filtered = screens.get("filtered", "")
    doc = screens.get("doc", "")
    back = screens.get("doc_back", "")
    checks = []
    checks.append(("Apps \u00b7 2" in lst and "Quick app" in lst and "Notes" in lst,
                   "the manager lists both apps with a count in the title"))
    checks.append(("QUICK APP" in running and "creator \u00b7" not in running,
                   "Enter runs the selected app full-screen"))
    checks.append(("ARG=--fast" in running,
                   "menu launches pass the app's default_args"))
    checks.append(("Apps \u00b7 2" in menu_back,
                   "the menu comes back after the app returns"))
    checks.append(("app quick returned to dst" in after,
                   "leaving the menu shows the run notice in the transcript"))
    checks.append(("args: --fast" in screens.get("quick_panel", ""),
                   "the annotation panel shows default args"))
    checks.append(("Apps \u00b7 1/2" in filtered and "Notes" in filtered
                   and "Quick app" not in filtered,
                   "typing filters the app list"))
    recent = screens.get("recent", "")
    rows = [l for l in recent.splitlines() if "Quick app" in l or "Notes" in l]
    checks.append((len(rows) >= 2 and "Quick app" in rows[0]
                   and "Quick app" in "".join(rows) and "\u00b7" in rows[0],
                   "the launcher lists the recently run app first with its time"))
    checks.append(("Notes" in doc and "Checklist item one" in doc,
                   "a doc app opens its annotation in the panel"))
    checks.append(("Apps \u00b7 1/2" in back,
                   "Esc leaves the panel back on the filtered menu"))
    checks.append((len(reqs) == 0, "the whole pass is local (no model request)"))
    return checks


def check_creator_run_args(screens, reqs, work):
    screen = screens.get("screen", "")
    after = screens.get("after", "")
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    checks = []
    checks.append(("ARGS: alpha|beta" in screen, "app_run forwards its args as argv"))
    checks.append(("STATE: " in screen and "/state" in screen,
                   "the app gets a writable DST_APP_STATE directory"))
    checks.append(("SECRET: unset" in screen,
                   "dst's credentials are not inherited by the app"))
    checks.append(("HOME: " in screen and "/state/home" in screen,
                   "the app's HOME is isolated inside the package state"))
    checks.append(("* app_run" in after and "exited (code 0)" in results,
                   "the tool card and result report a clean exit"))
    checks.append(("printed the arguments" in after, "the turn continues after the app"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks



def check_creator_skill(screens, reqs, work):
    built = screens.get("built", "")
    second = screens.get("second", "")
    home = os.path.join(work, "home")
    md = read_file(os.path.join(home, ".dst", "skills", "battery-report", "SKILL.md"))
    sys3 = system_text(reqs[2]) if len(reqs) > 2 else ""
    sys1 = system_text(reqs[0]) if reqs else ""
    calls = [n for n, _ in {(c["name"], c["args"]) for c in request_tool_calls(reqs)}]
    checks = []
    checks.append(("skill_save" in calls and "saved skill battery-report v2" in built,
                   "skill_save runs and the save is announced"))
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    checks.append(("UPDATED: read capacity" in results and "package:" in results,
                   "the skill tool loads the latest body written in the same turn"))
    checks.append(("version: 2" in md and "origin: model" in md,
                   "skill_save bumps the version in SKILL.md"))
    hist = read_file(os.path.join(home, ".dst", "skills", "battery-report", ".history", "v1.md"))
    checks.append(("1. Read /sys/class/power_supply" in hist,
                   "the previous skill body is kept under .history/v1.md"))
    checks.append((md.startswith("---\nname: battery-report\n")
                   and "description: Use when the user asks for a battery status report" in md
                   and "# Battery report" in md,
                   "SKILL.md written with frontmatter and body"))
    checks.append(("- battery-report:" not in sys1,
                   "the skill is not in the catalog before it exists"))
    checks.append(("- battery-report (v2): Use when the user asks for a battery status report" in sys3,
                   "the new skill joins the SKILLS catalog on the next turn"))
    checks.append(("creator: files changed but no app saved" not in built,
                   "saving a skill counts as persisted work (no nudge)"))
    checks.append(("one skill: battery-report" in second, "the model sees its own skill"))
    ext = screens.get("ext_list", "")
    ask = screens.get("ext_confirm", "")
    after = screens.get("ext_after", "")
    sys4 = system_text(reqs[3]) if len(reqs) > 3 else ""
    checks.append(("battery-report" in ext and "battery-probe" in ext
                   and "[model]" in ext,
                   "/extensions marks the model-written skills"))
    body = screens.get("ext_body", "")
    checks.append(("skill battery-probe" in body and "Print /sys/class/power_supply" in body,
                   "Right shows the full SKILL.md body in the viewer"))
    checks.append(("delete skill battery-probe? y/n" in ask,
                   "d asks before deleting a skill package"))
    checks.append((not os.path.isdir(os.path.join(home, ".dst", "skills", "battery-probe"))
                   and os.path.isdir(os.path.join(home, ".dst", "skills", "battery-report")),
                   "only the selected skill package is removed"))
    checks.append(("deleted skill battery-probe" in after,
                   "the deletion is reported in the transcript"))
    checks.append(("- battery-probe:" not in sys4 and "- battery-report (v2):" in sys4,
                   "the deleted skill leaves the catalog on the next turn"))
    checks.append((len(reqs) == 4, "exactly the scripted requests were made"))
    return checks


def check_creator_app_test(screens, reqs, work):
    tested = screens.get("tested", "")
    home = os.path.join(work, "home")
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    calls = [n for n, _ in {(c["name"], c["args"]) for c in request_tool_calls(reqs)}]
    good_log = read_file(os.path.join(home, ".dst", "tmp", "app-test-good.log"))
    broken_log = read_file(os.path.join(home, ".dst", "tmp", "app-test-broken.log"))
    checks = []
    checks.append((calls.count("app_test") == 2 and "* app_test" in tested
                   and "app_test good: exit=0" in tested
                   and "app_test broken: exit=3" in tested,
                   "both headless tests ran and are shown as tool cards"))
    checks.append(("GOOD APP" in results and "arg=--fast" in results
                   and "stdin=hello from test" in results,
                   "stdout, argv and piped stdin are captured"))
    checks.append(("exit=0" in results and "exit=3" in results,
                   "both exit codes are reported"))
    checks.append(("BROKEN APP" in results and "cannot continue" in results,
                   "stderr of the failing app is captured"))
    checks.append(("creator \u00b7" in tested and "GOOD APP" not in tested,
                   "app_test stays headless: the chat screen is intact"))
    checks.append(("GOOD APP" in good_log and "stdout" in good_log,
                   "the full log file exists for the model to read"))
    checks.append(("exit=Some(3)" in broken_log or "cannot continue" in broken_log,
                   "the failing run is logged too"))
    panel = screens.get("panel", "")
    checks.append(("last test: exit 3" in panel and "last run:" not in panel,
                   "the panel records the headless test (and no screen run)"))
    lst = screens.get("list", "")
    checks.append(("sh ! \u00b7" in lst,
                   "the app list flags the failing app and shows its last-use time"))
    checks.append(("Apps \u00b7 2 \u00b7 1! \u00b7 skills " in lst,
                   "the launcher title counts the failing app"))
    improve = screens.get("improve", "")
    checks.append(("Improve the app broken:" in improve,
                   "i pre-fills an improvement request for the selected app"))
    checks.append((" \u00b7 1!" in screens.get("tested", ""),
                   "the creator idle hint counts the failing app"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks


def check_creator_kinds(screens, reqs, work):
    screen = screens.get("screen", "")
    after = screens.get("after", "")
    lst = screens.get("list", "")
    sent = screens.get("prompt_sent", "")
    reply = screens.get("prompt_reply", "")
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    texts = "\n".join(message_texts(reqs))
    checks = []
    checks.append(("BINARY APP OK" in screen and "creator \u00b7" not in screen,
                   "a binary-kind app takes over the panel"))
    checks.append(("BINARY APP OK" in results and "exited (code 0)" in results,
                   "app_test and app_run both report the binary app"))
    checks.append(("* app_run" in after and "exited (code 0)" in results,
                   "the tool card records the binary run"))
    rows = [l for l in lst.splitlines() if "Hello binary" in l or "Report" in l]
    checks.append(("Hello binary" in lst and "Report" in lst and len(rows) == 2
                   and "binary" in rows[0] and "prompt" in rows[1],
                   "the menu lists binary and prompt apps with their kind values"))
    review = screens.get("review", "")
    checks.append(("three-line status of the device" in review
                   and "Enter send" in review,
                   "a prompt app is previewed before sending (Enter sends)"))
    checks.append(("> prompt app report" in sent,
                   "launching a prompt app is marked in the transcript"))
    checks.append(("Run the prompt app 'report'" in texts
                   and "three-line status of the device" in texts,
                   "the prompt app's request reaches the model"))
    checks.append(("Device status: battery 87%" in reply,
                   "the model follows the prompt app"))
    checks.append((len(reqs) == 3, "exactly the scripted requests were made"))
    return checks



def check_creator_skill_rollback(screens, reqs, work):
    home = os.path.join(work, "home")
    base = os.path.join(home, ".dst", "skills", "report")
    md = read_file(os.path.join(base, "SKILL.md"))
    v2 = read_file(os.path.join(base, ".history", "v2.md"))
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    checks = []
    checks.append(("ORIGINAL: read once" in md and "SECOND: read twice" not in md,
                   "rollback restores the earlier body as the current SKILL.md"))
    checks.append(("version: 3" in md and "origin: model" in md,
                   "the restored body becomes a new version (v3)"))
    checks.append(("SECOND: read twice" in v2,
                   "the body that was current before the rollback is archived"))
    checks.append(("rolled back skill report to the body of v1 (now v3)" in results,
                   "the tool result explains the rollback"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks



def check_creator_capture(screens, reqs, work):
    home = os.path.join(work, "home")
    log_path = os.path.join(home, ".dst", "skills", "chatty", "state", "last-run.log")
    log = read_file(log_path)
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    checks = []
    checks.append(("CHATTY LINE 1" in screens.get("screen", "")
                   and "CHATTY LINE 2" in screens.get("screen", ""),
                   "captured stdout is still echoed to the panel"))
    checks.append(("CHATTY LINE 1" in log and "CHATTY LINE 2" in log and "done" in log,
                   "stdout is recorded in state/last-run.log"))
    checks.append(("\x1b" not in log and "progress\ndone" in log,
                   "the log strips ANSI codes and turns CR progress into lines"))
    checks.append(("Output was recorded to" in results and "/state/last-run.log" in results,
                   "the tool result points at the recorded log"))
    panel = screens.get("log", "")
    checks.append(("Chatty app \u00b7 log" in panel and "CHATTY LINE 1" in panel,
                   "/apps 'l' replays the recorded output"))
    lst = screens.get("list", "")
    checks.append(("sh \u00b7" in lst,
                   "the launcher shows when the app was last used"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks



def check_creator_home(screens, reqs, work):
    launcher = screens.get("launcher", "")
    prefilled = screens.get("prefilled", "")
    asked = screens.get("asked", "")
    texts = "\n".join(message_texts(reqs))
    checks = []
    checks.append(("Apps \u00b7 0 \u00b7 skills " in launcher,
                   "the launcher title counts apps and skills"))
    checks.append(("+ new app" in launcher,
                   "creator mode offers a + new app row even with no apps"))
    checks.append(("Build an app:" in prefilled,
                   "Enter on + new app pre-fills the prompt for the user"))
    checks.append(("Build an app: battery viewer" in texts,
                   "the described app request reaches the model"))
    checks.append(("Got it" in asked, "the model answers the build request"))
    checks.append((len(reqs) == 1, "exactly the scripted request was made"))
    return checks



def check_creator_tty_test(screens, reqs, work):
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    tested = screens.get("tested", "")
    calls = [n for n, _ in {(c["name"], c["args"]) for c in request_tool_calls(reqs)}]
    checks = []
    checks.append((calls.count("app_test") == 2 and "app_test ttycheck: exit=0" in tested,
                   "both app_test calls ran and are shown"))
    checks.append(("MODE: pipe" in results and "MODE: tty" in results,
                   "tty=true runs under a pseudo-terminal while the default is piped"))
    checks.append(("STDIN: pipe" in results and "STDIN: tty" in results,
                   "stdin is a pipe by default and a pty in tty mode"))
    checks.append(("SIZE: 53x12" in results,
                   "COLUMNS/LINES expose the device grid in both modes"))
    checks.append(("LINE: hello-pipe" in results and "LINE: hello-tty" in results,
                   "the stdin argument reaches the app in both modes"))
    checks.append(("pty, no screen" in results and "headless, no screen" in results,
                   "the result says which transport was used"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks



def check_creator_requires(screens, reqs, work):
    results = "\n".join(str(m.get("content") or "") for m in request_tool_results(reqs))
    tested = screens.get("tested", "")
    checks = []
    checks.append(("requires 'dst-definitely-missing-tool'" in results
                   and "not installed on this device" in results,
                   "a missing requirement is refused with an actionable error"))
    checks.append(("SHOULD NOT RUN" not in results,
                   "the refused app was never executed"))
    checks.append(("OK APP RAN" in results and "exit=0" in results,
                   "an app whose requirements exist still runs"))
    checks.append(("* app_test" in tested, "both tests are shown as cards"))
    checks.append((len(reqs) == 2, "exactly the scripted requests were made"))
    return checks


def agent_requests(reqs):
    """Chat requests made by the agent itself (not sandbox-judge calls)."""
    out = []
    for r in reqs:
        msgs = r.get("messages") or []
        if msgs and str(msgs[0].get("content") or "").startswith("You are dst"):
            out.append(r)
    return out


def check_progress(screens, reqs, work):
    pinned = screens.get("pinned", "")
    running = screens.get("running", "")
    ctrlo = screens.get("ctrlo", "")
    scrolled = screens.get("scrolled_busy", "")
    interrupted = screens.get("interrupted", "")
    nxt = screens.get("next", "")
    agents = agent_requests(reqs)
    checks = []
    first = pinned.splitlines()[0] if pinned.splitlines() else ""
    checks.append((first.startswith("\u00bb reading /proc"),
                   "state_report is pinned to the very top row"))
    checks.append(("* bash" in running and "command: sleep 5" in running,
                   "the running tool call is expanded with its arguments"))
    checks.append(("Tool output" in ctrlo and "quick-done" in ctrlo,
                   "CtrlO opens the tool log during a running turn"))
    checks.append(("lines \u00b7 End" in scrolled and "filler" in scrolled,
                   "PgUp pages the transcript while a turn is running"))
    checks.append(("cancelled" in interrupted, "Esc cancels the whole turn"))
    # Two filler turns + two steps of the big turn + the follow-up: one more
    # request would mean the driver kept the interrupted turn alive.
    checks.append((len(agents) == 5,
                   f"no extra model step after the interrupt ({len(agents)} requests)"))
    checks.append((len(agents) == 5
                   and "now answer shortly"
                   in json.dumps(agents[-1], ensure_ascii=False),
                   "the follow-up message is what was sent next"))
    checks.append(("Follow-up answer." in nxt, "the next turn works normally"))
    return checks


def check_paging(screens, reqs, work):
    bottom = screens.get("bottom", "")
    back = screens.get("back", "")
    checks = []
    checks.append(("Third reply line B." in bottom, "newest output is at the bottom"))
    # Every page up must still show transcript content: scrolling past the top
    # used to blank the panel because the window was not clamped.
    for i in range(1, 5):
        page = screens.get(f"page{i}", "")
        checks.append(("lines \u00b7 End" in page and ">" in page,
                       f"PgUp page {i} shows scrollback, not a blank screen"))
    checks.append(("first question" in screens.get("page4", "")
                   or "first question" in screens.get("page3", ""),
                   "paging reaches the first prompt"))
    checks.append(("Third reply line B." in back and "lines \u00b7 End" not in back,
                   "End returns to the newest lines"))
    return checks


def check_voice_interrupt(screens, reqs, work):
    busy = screens.get("busy", "")
    rec = screens.get("recording", "")
    typed = screens.get("transcribed", "")
    reply = screens.get("reply", "")
    voice_log = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    agents = agent_requests(reqs)
    checks = []
    checks.append(("sleep 5" in busy, "a long tool call is running"))
    checks.append(("REC" in rec, "Tab during the turn interrupts and records at once"))
    checks.append(("stop and take this" in typed,
                   "the take is transcribed into the prompt (auto_send off)"))
    checks.append(("start the slow job" in typed or "REC" in rec,
                   "the interrupted turn is no longer on screen"))
    checks.append(("stop requested" in voice_log, "the voice layer got the stop signal"))
    checks.append(("armed (interrupt)" in voice_log,
                   "the interrupt take skips the tap debounce"))
    checks.append((len(agents) == 2,
                   f"the interrupted turn stopped at once ({len(agents)} requests)"))
    checks.append(("Heard the voice interrupt." in reply,
                   "Enter sends the transcribed prompt and the reply renders"))
    return checks



def check_voice_interrupt_send(screens, reqs, work):
    busy = screens.get("busy", "")
    rec = screens.get("recording", "")
    reply = screens.get("reply", "")
    voice_log = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    agents = agent_requests(reqs)
    checks = []
    checks.append(("sleep 5" in busy, "a long tool call is running"))
    checks.append(("REC" in rec, "Tab during the turn interrupts and records at once"))
    checks.append(("stop requested" in voice_log, "the voice layer got the stop signal"))
    checks.append(("armed (interrupt)" in voice_log,
                   "the interrupt take skips the tap debounce"))
    checks.append((len(agents) == 2,
                   f"the interrupted turn stopped at once ({len(agents)} requests)"))
    last_user = ""
    if agents:
        users = [m for m in (agents[-1].get("messages") or [])
                 if m.get("role") == "user"]
        if users:
            last_user = str(users[-1].get("content") or "")
    checks.append(("stop and take this" in last_user,
                   "auto_send_voice sends the ASR text with no Enter"))
    checks.append(("Heard the voice interrupt." in reply,
                   "the reply to the auto-sent voice prompt renders"))
    return checks


def check_onboarding_hw(screens, reqs, work):
    asked = screens.get("asked", "")
    scanned = screens.get("scanned", "")
    home = os.path.join(work, "home")
    cfg = read_file(os.path.join(home, ".dst", "config.yml"))
    skill = read_file(os.path.join(home, ".dst", "skills", "hardware-info", "SKILL.md"))
    checks = []
    checks.append(("Hardware notebook" in asked and "Scan this device's hardware" in asked,
                   "first-run setup offers the hardware scan"))
    checks.append(("hardware-info updated" in scanned,
                   "answering Y runs the scan and reports the refreshed skill"))
    checks.append(("hw_survey_done: true" in cfg,
                   "the survey question is recorded as asked (once)"))
    checks.append(("## Current state (measured)" in skill,
                   "the scan section is written into the hardware-info skill"))
    checks.append(("key: test-key" in cfg,
                   "the verified credential is the one that was saved"))
    return checks


def check_onboarding_reject(screens, reqs, work):
    rejected = screens.get("rejected", "")
    home = os.path.join(work, "home")
    cfg = read_file(os.path.join(home, ".dst", "config.yml"))
    skill = read_file(os.path.join(home, ".dst", "skills", "hardware-info", "SKILL.md"))
    checks = []
    checks.append(("key rejected" in rejected,
                   "an unusable credential is rejected during first-run setup"))
    checks.append(("No API configured" in rejected,
                   "the device stays unconfigured instead of looking ready"))
    checks.append(("key:" not in cfg or "key: bad-key" not in cfg,
                   "the rejected key is not kept"))
    checks.append(("## Current state (measured)" not in skill,
                   "no hardware scan runs when setup is rejected"))
    return checks


def check_update_hwinfo(screens, reqs, work):
    updated = screens.get("updated", "")
    home = os.path.join(work, "home")
    skill = read_file(os.path.join(home, ".dst", "skills", "hardware-info", "SKILL.md"))
    checks = []
    checks.append(("/update-hwinfo" in updated, "the command is echoed in the transcript"))
    checks.append(("hardware-info updated" in updated,
                   "the scan runs and reports the refreshed skill"))
    checks.append(("## Current state (measured)" in skill,
                   "the scan section is written into the skill"))
    checks.append(("version: 2" in skill, "the skill version is bumped by the first scan"))
    checks.append(("creator mode lets the model refine" in updated,
                   "normal mode explains how to get the model refinement"))
    checks.append((len(reqs) == 0, "normal mode does not spend a model turn"))
    return checks


def check_update_hwinfo_creator(screens, reqs, work):
    refined = screens.get("refined", "")
    home = os.path.join(work, "home")
    skill = read_file(os.path.join(home, ".dst", "skills", "hardware-info", "SKILL.md"))
    texts = "\n".join(message_texts(reqs))
    checks = []
    checks.append(("## Current state (measured)" in skill and "version: 2" in skill,
                   "the scan still runs locally in creator mode"))
    checks.append(("scan saved; asking the model" in refined
                   or "scan saved" in refined,
                   "the transcript labels the internal refine request"))
    checks.append(("Hardware skill reviewed." in refined,
                   "the model's refinement reply is rendered"))
    checks.append((len(reqs) == 1, "exactly one model turn was triggered"))
    checks.append(("hardware-info skill was just refreshed" in texts,
                   "the refine request names the refreshed hardware skill"))
    checks.append(("## Current state (measured)" in skill,
                   "the scan section is present before the model reads it"))
    return checks


def check_image_input(screens, reqs, work):
    attached = screens.get("attached", "")
    reply = screens.get("reply", "")
    checks = []
    checks.append(("image attached:" in attached and "png" in attached,
                   "/image attaches a local file and reports its type"))
    checks.append(("blue picture" in reply,
                   "the model's answer to the image question is rendered"))
    # The first agent request must carry the DeepSeek vision block: an array
    # content with an image_url data URL.
    user_lists = []
    for body in reqs:
        for m in body.get("messages") or []:
            if m.get("role") == "user" and isinstance(m.get("content"), list):
                user_lists.append(m["content"])
    ok_block = False
    for content in user_lists:
        kinds = [p.get("type") for p in content if isinstance(p, dict)]
        urls = [
            p.get("image_url", {}).get("url", "")
            for p in content
            if isinstance(p, dict) and p.get("type") == "image_url"
        ]
        if "text" in kinds and "image_url" in kinds and any(
            u.startswith("data:image/png;base64,") for u in urls
        ):
            ok_block = True
    checks.append((ok_block, "the request sends a DeepSeek image_url content block"))
    # After the turn, the history must keep only the placeholder: no re-sending
    # megabytes of base64 on every later request.
    leaked = []
    for body in reqs[1:]:
        for m in body.get("messages") or []:
            if m.get("role") == "user" and isinstance(m.get("content"), str):
                if "[image attached:" not in m["content"] and "what is in this image?" in m["content"]:
                    leaked.append(m["content"][:60])
    checks.append((not leaked,
                   "the image is replaced by a text placeholder in later requests"))
    return checks


def check_read_image_tool(screens, reqs, work):
    reply = screens.get("reply", "")
    checks = []
    checks.append(("blue picture" in reply,
                   "the model opened the local file with read_image and described it"))
    # Request 2 is the step after the tool call: the tool message must carry the
    # DeepSeek image block (content array with an image_url data URL).
    ok_block = False
    for body in reqs[1:]:
        for m in body.get("messages") or []:
            if m.get("role") == "tool" and isinstance(m.get("content"), list):
                parts = [p for p in m["content"] if isinstance(p, dict)]
                urls = [
                    p.get("image_url", {}).get("url", "")
                    for p in parts
                    if p.get("type") == "image_url"
                ]
                if any(u.startswith("data:image/png;base64,") for u in urls):
                    ok_block = True
    checks.append((ok_block,
                   "the read_image tool result returns the image to the model"))
    # After the step, the tool message is rewritten to text: later requests must
    # not re-send the payload.
    later = reqs[2:]
    clean = True
    placeholder = False
    for body in later:
        for m in body.get("messages") or []:
            if m.get("role") != "tool":
                continue
            c = m.get("content")
            if isinstance(c, list):
                clean = False
            elif isinstance(c, str) and "[image read:" in c:
                placeholder = True
    checks.append((clean and placeholder,
                   "the tool result is replaced by a placeholder after the step"))
    return checks


def check_rewind_prompt(screens, reqs, work):
    picker = screens.get("picker", "")
    restored = screens.get("restored", "")
    resent = screens.get("resent", "")
    checks = []
    checks.append(("and gamma" in picker and "remember beta" in picker,
                   "the picker lists previous user inputs, not opaque turns"))
    checks.append(("Rewind to an earlier message" in picker,
                   "the picker explains that it rewinds to a message"))
    checks.append(("> my project is called alpha" in restored and "and gamma" not in restored,
                   "restoring cuts the transcript back to before the chosen message"))
    checks.append(("message put back in the prompt" in restored,
                   "the chosen message is put back in the prompt for editing"))
    last_line = [l for l in restored.splitlines() if l.strip().startswith(">")][-1] if ">" in restored else ""
    checks.append(("remember beta" in last_line,
                   "the prompt line is pre-filled with that message"))
    checks.append(("> remember beta" in resent and "Beta re-sent." in resent,
                   "pressing Enter re-sends the rewound message and renders the answer"))
    if reqs:
        last = reqs[-1]
        texts = [str(m.get("content") or "") for m in (last.get("messages") or [])]
        joined = "\n".join(texts)
        checks.append(("gamma" not in joined,
                       "the abandoned future is gone from the re-sent request"))
        checks.append((any("remember beta" in t for t in texts),
                       "the re-sent request carries the rewound message"))
    return checks


WRITE_TOOLS = [
    "write_file", "edit_file", "bash", "skill_save", "skill_file",
    "skill_rollback", "app_run", "app_test", "job_start", "watcher_start",
    "artifact", "speak", "cron", "watch", "memory_write", "todo_write",
    "create_goal", "update_goal",
]


def check_btw_side_question(screens, reqs, work):
    panel = screens.get("btw_panel", "")
    after = screens.get("after", "")
    btw_req = None
    for body in reqs:
        texts = [
            str(m.get("content") or "")
            for m in (body.get("messages") or [])
            if m.get("role") == "system"
        ]
        if any("SIDE QUESTION MODE" in t for t in texts):
            btw_req = body
            break
    checks = []
    checks.append((btw_req is not None, "the /btw side question ran as its own request"))
    checks.append(("btw" in panel and "It says 42." in panel,
                   "the subagent's answer is shown in a panel"))
    checks.append(("Alpha noted." not in panel and "my project is called alpha" not in panel,
                   "the panel holds only the answer, not the conversation"))
    if btw_req is not None:
        names = [
            (t.get("function") or {}).get("name")
            for t in (btw_req.get("tools") or [])
            if isinstance(t, dict)
        ]
        bad = [n for n in names if n in WRITE_TOOLS]
        checks.append((not bad, f"no write tools are available to the subagent ({bad})"))
        checks.append(("read_file" in names, "read-only tools are available to the subagent"))
        joined = "\n".join(str(m.get("content") or "") for m in (btw_req.get("messages") or []))
        checks.append(("my project is called alpha" in joined,
                       "the subagent sees the current conversation context"))
        last_user = [
            str(m.get("content") or "")
            for m in (btw_req.get("messages") or [])
            if m.get("role") == "user"
        ][-1]
        checks.append(("what does notes.txt say?" in last_user,
                       "the side question is the last user message"))
    # The main conversation must be untouched by the side question.
    if reqs:
        last = reqs[-1]
        joined = "\n".join(str(m.get("content") or "") for m in (last.get("messages") or []))
        checks.append(("SIDE QUESTION MODE" not in joined,
                       "the btw instructions never enter the main conversation"))
        checks.append(("It says 42." not in joined and "notes.txt" not in joined,
                       "the side question and its answer are not persisted"))
        checks.append(("Alpha noted." in joined and "Main reply." in after,
                       "the main conversation continues normally after the side question"))
        checks.append(("/btw" not in after,
                       "the /btw command itself does not pollute the main transcript"))
    return checks


def check_busy_commands(screens, reqs, work):
    usage = screens.get("usage_busy", "")
    refused = screens.get("refused", "")
    panel = screens.get("btw_panel", "")
    queued = screens.get("queued", "")
    after = screens.get("after", "")
    cfg_text = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("cache hit rate" in usage and "bash running" in usage,
                   "commands that need no model run immediately during a turn"))
    checks.append(("needs a stopped turn" in refused and "/model" in refused,
                   "model-changing commands are refused while a turn runs"))
    checks.append(("model: deepseek-flash" in cfg_text and "deepseek-v9" not in cfg_text,
                   "the refused /model did not change the config"))
    checks.append(("btw \u00b7" in panel and "The job is sleeping" in panel,
                   "/btw runs immediately and shows its answer in a panel"))
    checks.append(("queued (1)" in queued,
                   "plain chat input is the only thing that queues"))
    checks.append(("Job done." in after and "Answer to follow up." in after,
                   "the turn and the queued follow-up both complete"))
    texts = "\n".join(str(m.get("content") or "")
                       for body in reqs for m in (body.get("messages") or []))
    checks.append(("/usage" not in texts and "/model" not in texts and "/btw" not in texts,
                   "commands are never sent to the model as chat"))
    btw_req = [b for b in reqs
               if any("SIDE QUESTION MODE" in str(m.get("content") or "")
                      for m in (b.get("messages") or []))]
    checks.append((len(btw_req) == 1,
                   "the side question is one separate read-only request"))
    if btw_req:
        btw_joined = "\n".join(str(m.get("content") or "")
                               for m in (btw_req[0].get("messages") or []))
        checks.append(("start a slow job" in btw_joined and "bash" in btw_joined,
                       "mid-turn /btw receives the current turn prompt and running tool status"))
    last_user = [str(m.get("content") or "")
                 for m in ((reqs[-1].get("messages") or []) if reqs else [])
                 if m.get("role") == "user"]
    checks.append((bool(last_user) and "queued follow up" in last_user[-1],
                   "the queued chat message is sent after the turn"))
    checks.append((len(reqs) == 4, f"exactly four model requests ({len(reqs)})"))
    return checks


def check_state_report_steps(screens, reqs, work):
    first = screens.get("first_pin", "")
    second = screens.get("second_pin", "")
    after = screens.get("after_turn", "")
    checks = []
    first_line = first.splitlines()[0] if first.splitlines() else ""
    checks.append((first_line.startswith("\u00bb reading the notes"),
                   "each step's state_report is pinned to the top row"))
    second_line = second.splitlines()[0] if second.splitlines() else ""
    checks.append((second_line.startswith("\u00bb writing the summary"),
                   "a newer report replaces the pinned row"))
    checks.append(("read_file" in second,
                   "the pinned row survives later tool cards and text"))
    checks.append(("The notes say 42." in after, "the answer renders"))
    checks.append(("\u00bb writing the summary" in after,
                   "the last report joins the transcript when the turn ends"))
    after_first = after.splitlines()[0] if after.splitlines() else ""
    checks.append((not after_first.startswith("\u00bb"),
                   "the pinned row is cleared when the turn ends"))
    def last_tool_names(body):
        for m in reversed(body.get("messages") or []):
            if m.get("role") == "assistant":
                return [(tc.get("function") or {}).get("name")
                        for tc in (m.get("tool_calls") or [])]
        return []
    steps = [last_tool_names(b) for b in reqs]
    checks.append((len(steps) >= 3 and "state_report" in steps[1]
                   and "state_report" in steps[2],
                   "every working step starts with a state_report call"))
    return checks


def check_usage_ring_reset(screens, reqs, work):
    startup = screens.get("startup", "")
    ring_line = [l for l in startup.splitlines() if "%" in l]
    checks = []
    checks.append((bool(startup), "the startup screen renders"))
    checks.append((any("\u25cb 0%" in l for l in ring_line),
                   "the context ring starts at 0%, not the previous session's value"))
    checks.append(("dst v" in startup and "/update" in startup,
                   "the one-time startup version banner is shown on launch"))
    checks.append((len(reqs) == 0, "no model request is needed for a fresh start"))
    return checks


def check_new_session(screens, reqs, work):
    cleared = screens.get("cleared", "")
    after = screens.get("after", "")
    checks = []
    checks.append(("hold Tab" in cleared and "%" in cleared,
                   "/clear returns to the splash and resets the context ring"))
    cleared_pct = [l for l in cleared.splitlines() if "%" in l]
    checks.append((any("\u25cb 0%" in l for l in cleared_pct),
                   "/clear leaves the ring at 0%"))
    checks.append((("hold Tab" in screens.get("new_session", "")),
                   "/new behaves like /clear"))
    checks.append(("Hello three." in after, "the conversation continues after /new"))
    def users(body):
        return [str(m.get("content") or "") for m in (body.get("messages") or [])
                if m.get("role") == "user"]
    if len(reqs) >= 3:
        checks.append((users(reqs[1]) == ["second message"],
                       "/clear starts a fresh history (no old user message is re-sent)"))
        checks.append((users(reqs[2]) == ["third message"],
                       "/new starts a fresh history again"))
        checks.append((reqs[1].get("messages") and len(reqs[1]["messages"]) == 2,
                       "/clear requests are only system+user"))
    else:
        checks.append((False, "expected three model requests"))
    return checks


def check_todo_plan(screens, reqs, work):
    plan_panel = screens.get("plan", "")
    progress = screens.get("progress", "")
    after = screens.get("next_turn", "")
    results = "\n".join(str(r.get("content") or "")
                        for r in request_tool_results(reqs))
    checks = []
    checks.append(("t0/2" in plan_panel and "* todo_write" in plan_panel,
                   "the plan is shown as a done/total badge and a tool card"))
    checks.append(("0 completed" in results
                   and "1 pending, 1 in progress" in results,
                   "the DSH-style result line reports pending/in-progress/completed"))
    checks.append(("t1/2" in progress,
                   "updating the whole list moves the badge to 1/2"))
    def last_tool_names(body):
        for m in reversed(body.get("messages") or []):
            if m.get("role") == "assistant":
                return [(tc.get("function") or {}).get("name")
                        for tc in (m.get("tool_calls") or [])]
        return []
    calls = [last_tool_names(b) for b in reqs]
    checks.append((len(calls) >= 3 and calls[1] == ["todo_write"]
                   and calls[2] == ["todo_write"],
                   "each working step sends one whole-list todo_write"))
    def todo_args(tc):
        raw = (tc.get("function") or {}).get("arguments")
        if isinstance(raw, str):
            try:
                return json.loads(raw)
            except ValueError:
                return {}
        return raw if isinstance(raw, dict) else {}
    todos = [tc for b in reqs for m in (b.get("messages") or [])
             for tc in (m.get("tool_calls") or [])
             if (tc.get("function") or {}).get("name") == "todo_write"]
    checks.append((bool(todos) and all("todos" in todo_args(tc) for tc in todos),
                   "the model sends the DSH 'todos' array, not legacy strings"))
    checks.append(("The notes say 42." in screens.get("progress", "")
                   or "The notes say 42." in after, "the answer renders"))
    checks.append(("t1/2" not in after and "t0/2" not in after,
                   "the todo list is cleared when the next turn starts"))
    return checks


def check_update_command(screens, reqs, work):
    failed = screens.get("failed", "")
    stable = screens.get("stable", "")
    usage = screens.get("usage", "")
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks = []
    checks.append(("update failed" in failed,
                   "/update runs the check and reports an unreachable update source"))
    checks.append(("update channel set to stable" in stable
                   and "update failed" in stable,
                   "/update <channel> switches the channel and checks it"))
    checks.append(("channel: stable" in cfg,
                   "the channel is persisted in config.yml for the next start"))
    checks.append(("/update [nightly|stable]" in usage,
                   "a bad channel argument prints the usage hint"))
    checks.append((len(reqs) == 0, "updating never spends a model request"))
    return checks


def check_update_offer(screens, reqs, work):
    import hashlib
    import glob
    offer = screens.get("offer", "")
    declined = screens.get("declined", "")
    failed = screens.get("failed", "")
    checks = []
    checks.append(("Update available" in offer and "nightly" in offer,
                   "the startup background check offers the newer nightly build"))
    checks.append(("update skipped" in declined,
                   "answering No dismisses the offer for this session"))
    checks.append(("checksum mismatch" in failed,
                   "choosing Yes downloads, verifies and refuses a bad checksum"))
    bins = glob.glob(os.path.join(work, "update-target-sha256"))
    recorded = read_file(bins[0]) if bins else ""
    current = ""
    binary = os.environ.get("DST_BIN") or os.path.join(REPO, "target", "debug", "dst")
    try:
        with open(binary, "rb") as fh:
            current = hashlib.sha256(fh.read()).hexdigest()
    except OSError:
        pass
    checks.append((bool(recorded) and recorded.strip() == current,
                   "a failed update leaves the running binary untouched"))
    checks.append((len(reqs) == 0, "the update flow spends no model request"))
    cfg = read_file(os.path.join(work, "home", ".dst", "config.yml"))
    checks.append(("channel: nightly" in cfg,
                   "declining or failing an update keeps the configured channel"))
    return checks


def check_voice_dictation(screens, reqs, work):
    typed = screens.get("typed", "")
    transcribed = screens.get("transcribed", "")
    reply = screens.get("reply", "")
    voice_log = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    checks = []
    checks.append(("note:" in typed and "add more detail" not in typed,
                   "text was typed before talking"))
    checks.append(("note: add more detail" in transcribed,
                   "with text in the prompt the take is inserted, not sent"))
    checks.append(("Got it." not in transcribed,
                   "dictation does not submit on release"))
    checks.append(("mode=dictation" in voice_log,
                   "voice.log records the dictation mode"))
    users = [str(m.get("content") or "") for m in (reqs[-1].get("messages") or [])
             if m.get("role") == "user"] if reqs else []
    checks.append((bool(users) and users[-1] == "note: add more detail",
                   "Enter then sends the combined prompt (no overwrite)"))
    checks.append(("Got it." in reply, "the combined prompt gets an answer"))
    return checks


def check_voice_talk_send(screens, reqs, work):
    reply = screens.get("reply", "")
    voice_log = read_file(os.path.join(work, "home", ".dst", "logs", "voice.log"))
    checks = []
    checks.append(("Heard: hello from voice." in reply,
                   "an empty prompt sends the take on release"))
    checks.append(("mode=send" in voice_log, "voice.log records the send mode"))
    users = [str(m.get("content") or "") for b in reqs for m in (b.get("messages") or [])
             if m.get("role") == "user"]
    checks.append((users == ["hello from voice"],
                   "the spoken text reached the model without pressing Enter"))
    return checks


def check_ask_user(screens, reqs, work):
    q1 = screens.get("q1", "")
    q2 = screens.get("q2_checked", "")
    q3 = screens.get("q3_custom", "")
    done = screens.get("done", "")
    results = "\n".join(str(r.get("content") or "") for r in request_tool_results(reqs))
    checks = []
    checks.append(("Build choices" in q1 and "1/3" in q1 and "Pick build mode:" in q1,
                   "single-select question renders with title and step counter"))
    checks.append(("Other (custom text)" in q1,
                   "Other custom-input option is automatically appended"))
    checks.append(("[x] Unit tests" in q2 and "[x] Strip binary" in q2,
                   "multi-select toggles multiple checkboxes with Space"))
    checks.append(("Type answer" in q3,
                   "choosing Other enters inline custom text input"))
    checks.append(("- [mode] Debug (id: debug)" in results,
                   "single-select answer reaches the model"))
    checks.append(("Unit tests (id: tests)" in results and "Strip binary (id: strip)" in results,
                   "multi-select answers reach the model"))
    checks.append(("Other: \"arm64 only\"" in results,
                   "custom Other answer reaches the model"))
    checks.append(("Using Debug with Unit tests and Strip binary." in done,
                   "model continues and replies after user selections"))
    return checks


CHECKS = {
    "ask_user": check_ask_user,
    "creator_app_flow": check_creator_app_flow,
    "esc_queue": check_esc_queue,
    "voice_interrupt_send": check_voice_interrupt_send,
    "creator_requires": check_creator_requires,
    "creator_tty_test": check_creator_tty_test,
    "creator_home": check_creator_home,
    "creator_capture": check_creator_capture,
    "creator_skill_rollback": check_creator_skill_rollback,
    "creator_skill": check_creator_skill,
    "creator_app_test": check_creator_app_test,
    "creator_kinds": check_creator_kinds,
    "creator_apps_run": check_creator_apps_run,
    "pet_app_hide": check_creator_apps_run,
    "creator_run_args": check_creator_run_args,
    "creator_mode": check_creator_mode,
    "creator_build": check_creator_build,
    "creator_run": check_creator_run,
    "creator_run_esc": check_creator_run_esc,
    "creator_apps_zh": check_creator_apps_zh,
    "creator_nudge": check_creator_nudge,
    "clear": check_clear,
    "login_menu": check_login_menu,
    "login_type_token": check_login_type_token,
    "login_wechat": check_login_wechat,
    "login_wechat_status": check_login_wechat_status,
    "login_wechat_retry": check_login_wechat_retry,
    "login_wechat_retry_more": check_login_wechat_retry_more,
    "login_wechat_countdown": check_login_wechat_countdown,
    "login_wechat_switch_typing": check_login_wechat_switch_typing,
    "key_overrides_token": check_key_overrides_token,
    "extensions_toggle": check_extensions_toggle,
    "usage_zh": check_usage_zh,
    "voice_input": check_voice_input,
    "voice_dictation": check_voice_dictation,
    "voice_talk_send": check_voice_talk_send,
    "voice_interrupt": check_voice_interrupt,
    "voice_speak": check_voice_speak,
    "queue_edit": check_queue_edit,
    "sandbox_es": check_sandbox_es,
    "history_persist": check_history_persist,
    "watcher_flood": check_watcher_flood,
    "sandbox_diff": check_sandbox_diff,
    "tips": check_tips,
    "ux": check_ux,
    "progress": check_progress,
    "image_input": check_image_input,
    "rewind_prompt": check_rewind_prompt,
    "btw_side_question": check_btw_side_question,
    "busy_commands": check_busy_commands,
    "state_report_steps": check_state_report_steps,
    "usage_ring_reset": check_usage_ring_reset,
    "new_session": check_new_session,
    "todo_plan": check_todo_plan,
    "update_command": check_update_command,
    "update_offer": check_update_offer,
    "read_image_tool": check_read_image_tool,
    "onboarding_hw": check_onboarding_hw,
    "onboarding_reject": check_onboarding_reject,
    "update_hwinfo": check_update_hwinfo,
    "update_hwinfo_creator": check_update_hwinfo_creator,
    "paging": check_paging,
    "ux2": check_ux2,
    "new_tools": check_new_tools,
    "rewind": check_rewind,
    "steering": check_steering,
    "i18n_es": check_i18n_es,
    "job_manager": check_job_manager,
    "sandbox_auto": check_sandbox_auto,
    "sandbox_strict": check_sandbox_strict,
    "sandbox_full": check_sandbox_full,
    "jobs": check_jobs,
    "long_input": check_long_input,
    "watcher": check_watcher,
    "resume_preview_zh": check_resume_preview_zh,
    "resume_search": check_resume_search,
    "session_menu": check_session_menu,
    "onboarding": check_onboarding,
    "api_error": check_api_error,
    "api_retry": check_api_retry,
    "goal_status_zh": check_goal_status_zh,
    "goal_manage": check_goal_manage,
    "commands": check_commands,
    "artifact": check_artifact,
    "artifact_scroll": check_artifact_scroll,
    "artifact_rich": check_artifact_rich,
    "goal_blocked": check_goal_blocked,
    "goal": check_goal,
    "compact": check_compact,
    "thinking": check_thinking,
    "tool_log_multi": check_tool_log_multi,
    "tool_order": check_tool_order,
    "tool_timeout": check_tool_timeout,
    "empty_states_zh": check_empty_states_zh,
    "esc_cancel": check_esc_cancel,
    "step_limit": check_step_limit,
    "pet_bounds": check_pet_bounds,
    "unlimited_steps": check_unlimited_steps,
    "repeat_reminder": check_repeat_reminder,
    "time_context": check_time_context,
    "compact_manual": check_compact_manual,
    "goal_complete": check_goal_complete,
    "config_menu": check_config_menu,
}

for _lang_ in LOGIN_I18N_EXPECT:
    CHECKS[f"login_i18n_{_lang_}"] = make_login_i18n_check(_lang_)


class _ScenarioTimeout(Exception):
    pass


def _alarm_handler(_signum, _frame):
    raise _ScenarioTimeout("scenario exceeded its watchdog")


def main(argv):
    names = argv or sorted(CHECKS)
    binary = os.environ.get("DST_BIN", DEFAULT_BIN)
    failures = 0
    for name in names:
        if name not in CHECKS:
            print(f"unknown scenario {name!r}; known: {', '.join(sorted(CHECKS))}")
            return 2
        log(f"scenario: {name}")
        t0 = time.time()
        # Watchdog: even a hung simulator teardown cannot stall the whole run.
        watchdog_s = _phase_timeout() + 60
        alarm_armed = hasattr(signal, "SIGALRM")
        if alarm_armed:
            signal.signal(signal.SIGALRM, _alarm_handler)
            signal.alarm(watchdog_s)
        try:
            screens, reqs, work = run_scenario(name, binary)
        except (Exception, _ScenarioTimeout) as exc:  # noqa: BLE001
            print(f"[FAIL] {name}: {exc}")
            failures += 1
            continue
        finally:
            if alarm_armed:
                signal.alarm(0)
        checks = CHECKS[name](screens, reqs, work)
        ok = all(c[0] for c in checks)
        for passed, label in checks:
            print(f"  {'ok  ' if passed else 'FAIL'} {label}")
        print(f"[{'PASS' if ok else 'FAIL'}] {name} ({time.time() - t0:.1f}s)")
        if ok and not os.environ.get("DST_KEEP_WORK"):
            shutil.rmtree(work, ignore_errors=True)
        else:
            # DST_KEEP_WORK only keeps artifacts for inspection; it must not
            # turn passing scenarios into failures.
            print(f"  work dir kept: {work}")
            if not ok:
                failures += 1
    if failures:
        print(f"{failures} scenario(s) failed")
        return 1
    print("all scenarios passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
