#!/usr/bin/env python3
"""End-to-end breakpoint test against a REAL Ren'Py SDK.

Skipped (exit 0) when no SDK is present — CI has none; run locally.
A game window will open briefly. The test project is a copy of the SDK's
the_question with an extra script whose init python block and splashscreen
label execute during startup, so breakpoints hit without touching the UI.

Usage: python3 tests/e2e_debug_real.py [path-to-server-binary]
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import threading

HERE = pathlib.Path(__file__).parent
DEFAULT_SERVER = HERE.parent / "server" / "target" / "release" / "renpy-language-server"
SERVER = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_SERVER


def find_sdk():
    env = os.environ.get("RENPY_SDK")
    if env and (pathlib.Path(env) / "renpy.py").is_file():
        return pathlib.Path(env)
    home = pathlib.Path.home()
    candidates = []
    for parent in (home, home / "Documents", home / "Downloads", home / "Desktop"):
        if parent.is_dir():
            for child in parent.iterdir():
                if "renpy" in child.name.lower() and (child / "renpy.py").is_file():
                    candidates.append(child)
    return sorted(candidates)[-1] if candidates else None


SDK = find_sdk()
if SDK is None:
    print("SKIP: no Ren'Py SDK found on this machine")
    sys.exit(0)

TEST_SCRIPT = """\
init python:
    zz_test_alpha = 1
    zz_test_beta = zz_test_alpha + 1
    zz_test_gamma = zz_test_beta + 1

label splashscreen:
    $ zz_splash = 42
    $ zz_splash_2 = zz_splash + 1
    return
"""

workdir = pathlib.Path(tempfile.mkdtemp(prefix="zed-renpy-debug-e2e-"))
project = workdir / "proj"
shutil.copytree(SDK / "the_question", project)
# A pristine copy compiles from source; stale .rpyc from the SDK copy is fine.
test_file = project / "game" / "zz_debug_test.rpy"
test_file.write_text(TEST_SCRIPT)

failures = []


def check(label, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"{status}: {label}" + (f"  [{detail}]" if detail and not condition else ""))
    if not condition:
        failures.append(label)


proc = subprocess.Popen(
    [str(SERVER), "dap"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
killer = threading.Timer(180, proc.kill)
killer.daemon = True
killer.start()
seq = [0]
events = []


def send(command, arguments=None):
    seq[0] += 1
    msg = {"seq": seq[0], "type": "request", "command": command}
    if arguments is not None:
        msg["arguments"] = arguments
    body = json.dumps(msg).encode()
    proc.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    proc.stdin.flush()
    return seq[0]


def read():
    headers = {}
    while True:
        line = proc.stdout.readline().decode()
        if line in ("\r\n", "\n", ""):
            break
        key, _, value = line.partition(":")
        headers[key.strip().lower()] = value.strip()
    if "content-length" not in headers:
        raise EOFError("adapter closed its stdout")
    return json.loads(proc.stdout.read(int(headers["content-length"])))


def response(rid):
    while True:
        msg = read()
        if msg.get("type") == "event":
            events.append(msg)
            continue
        if msg.get("type") == "response" and msg.get("request_seq") == rid:
            return msg


def next_stopped():
    while True:
        msg = read()
        if msg.get("type") == "event":
            events.append(msg)
            if msg["event"] == "stopped":
                return msg
            if msg["event"] == "terminated":
                return None


def stack():
    rid = send("stackTrace", {"threadId": 1})
    return response(rid).get("body", {}).get("stackFrames", [])


# --- session setup: breakpoints land before launch
rid = send("initialize", {"adapterID": "renpy"})
response(rid)
rid = send("setBreakpoints", {
    "source": {"path": str(test_file)},
    "breakpoints": [{"line": 3}, {"line": 7}],
})
resp = response(rid)
check(
    "breakpoints reported verified",
    all(b.get("verified") for b in resp["body"]["breakpoints"]),
    json.dumps(resp),
)
rid = send("configurationDone")
response(rid)
rid = send("launch", {"sdk": str(SDK), "project": str(project)})
check("launch succeeds", response(rid).get("success") is True)
check("debug agent file injected", (project / "game" / "zed_debug.rpe.py").is_file())

# --- stop 1: python-line breakpoint inside init python, during startup
stop = next_stopped()
check("stopped at python breakpoint", stop is not None and stop["body"]["reason"] == "breakpoint",
      json.dumps(stop))
frames = stack()
check(
    "top frame is the init python line",
    frames and frames[0]["line"] == 3 and frames[0]["source"]["path"] == str(test_file),
    json.dumps(frames[:2]),
)
check(
    "stack includes the Ren'Py statement frame",
    any(f["name"].startswith("statement:") for f in frames),
    json.dumps(frames),
)

rid = send("scopes", {"frameId": 0})
scopes = response(rid)["body"]["scopes"]
scope_names = [s["name"] for s in scopes]
check("scopes offered (Locals/Store)", "Locals" in scope_names, json.dumps(scopes))

found_alpha = None
for scope in scopes:
    rid = send("variables", {"variablesReference": scope["variablesReference"]})
    for var in response(rid)["body"]["variables"]:
        if var["name"] == "zz_test_alpha":
            found_alpha = var
check(
    "zz_test_alpha visible with value 1",
    found_alpha is not None and found_alpha["value"] == "1",
    json.dumps(found_alpha),
)

rid = send("evaluate", {"expression": "zz_test_alpha + 100", "context": "repl"})
resp = response(rid)
check("evaluate in paused frame", resp.get("success") and resp["body"]["result"] == "101",
      json.dumps(resp))

# --- step: next python line in the same block
rid = send("next", {"threadId": 1})
response(rid)
stop = next_stopped()
check("step stops", stop is not None and stop["body"]["reason"] == "step", json.dumps(stop))
frames = stack()
check("step landed on line 4", frames and frames[0]["line"] == 4, json.dumps(frames[:1]))

# --- stop 2: statement breakpoint on the splashscreen $ line
rid = send("continue", {"threadId": 1})
response(rid)
stop = next_stopped()
check("stopped at statement breakpoint", stop is not None and stop["body"]["reason"] == "breakpoint",
      json.dumps(stop))
frames = stack()
check(
    "statement stop is at splashscreen line 7",
    frames and frames[0]["line"] == 7 and frames[0]["source"]["path"] == str(test_file),
    json.dumps(frames[:2]),
)

# --- statement-level step
rid = send("next", {"threadId": 1})
response(rid)
stop = next_stopped()
frames = stack() if stop else []
check(
    "statement step lands on line 8",
    stop is not None and frames and frames[0]["line"] == 8,
    json.dumps(frames[:1]),
)

rid = send("evaluate", {"expression": "zz_splash", "context": "repl"})
resp = response(rid)
check("evaluate against the store", resp.get("success") and resp["body"]["result"] == "42",
      json.dumps(resp))

# --- run on, then shut everything down
rid = send("continue", {"threadId": 1})
response(rid)
import time
time.sleep(2)
rid = send("terminate", {})
response(rid)
while True:
    msg = read()
    if msg.get("type") == "event":
        events.append(msg)
        if msg["event"] == "terminated":
            break
rid = send("disconnect", {})
response(rid)
proc.wait(timeout=15)
check("adapter exits cleanly", proc.returncode == 0, str(proc.returncode))
check("injected agent file cleaned up", not (project / "game" / "zed_debug.rpe.py").exists())

# --- scenario 2: a breakpoint on a story DIALOGUE line, reached via warp
# (the_question's first narrator line; no UI interaction needed)
story = project / "game" / "script.rpy"
proc = subprocess.Popen(
    [str(SERVER), "dap"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
killer = threading.Timer(180, proc.kill)
killer.daemon = True
killer.start()
seq[0] = 0
events.clear()

rid = send("initialize", {"adapterID": "renpy"})
response(rid)
rid = send("setBreakpoints", {
    "source": {"path": str(story)},
    "breakpoints": [{"line": 18}],
})
response(rid)
rid = send("configurationDone")
response(rid)
rid = send("launch", {
    "sdk": str(SDK),
    "project": str(project),
    "warp": f"{story}:18",
})
check("warp launch succeeds", response(rid).get("success") is True)

stop = next_stopped()
check(
    "stopped on a dialogue line",
    stop is not None and stop["body"]["reason"] == "breakpoint",
    json.dumps(stop),
)
frames = stack()
check(
    "top frame is the say statement at line 18",
    frames and frames[0]["line"] == 18
    and frames[0]["source"]["path"] == str(story)
    and "say" in frames[0]["name"],
    json.dumps(frames[:2]),
)
rid = send("evaluate", {"expression": "config.name", "context": "repl"})
resp = response(rid)
check(
    "evaluate engine state at a dialogue pause",
    resp.get("success") and "Question" in resp["body"]["result"],
    json.dumps(resp),
)

# (No step test here: after resuming, the say waits for a player click, so
# the next statement wouldn't run without UI interaction. Statement stepping
# is covered by the splashscreen scenario above.)
rid = send("continue", {"threadId": 1})
response(rid)
time.sleep(2)
rid = send("terminate", {})
response(rid)
while True:
    msg = read()
    if msg.get("type") == "event":
        events.append(msg)
        if msg["event"] == "terminated":
            break
rid = send("disconnect", {})
response(rid)
proc.wait(timeout=15)
check("warp-session adapter exits cleanly", proc.returncode == 0, str(proc.returncode))

shutil.rmtree(workdir, ignore_errors=True)

if failures:
    print(f"\n{len(failures)} FAILURE(S)")
    sys.exit(1)
print("\nALL CHECKS PASSED")
