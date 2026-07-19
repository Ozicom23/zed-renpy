#!/usr/bin/env python3
"""End-to-end test: drive `renpy-language-server dap` over real DAP stdio.

Uses the fake SDK in tests/fixture-sdk (no real Ren'Py needed).
Usage: python3 tests/e2e_dap.py [path-to-server-binary]
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import time

HERE = pathlib.Path(__file__).parent
FIXTURE = HERE / "fixture"
FAKE_SDK = HERE / "fixture-sdk"
DEFAULT_SERVER = HERE.parent / "server" / "target" / "release" / "renpy-language-server"
SERVER = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_SERVER

failures = []


def check(label, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"{status}: {label}" + (f"  [{detail}]" if detail and not condition else ""))
    if not condition:
        failures.append(label)


class Dap:
    """One adapter process and its message plumbing."""

    def __init__(self, env=None):
        self.proc = subprocess.Popen(
            [str(SERVER), "dap"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env=env,
        )
        self.seq = 0
        self.events = []
        killer = threading.Timer(120, self.proc.kill)
        killer.daemon = True
        killer.start()

    def send(self, command, arguments=None):
        self.seq += 1
        msg = {"seq": self.seq, "type": "request", "command": command}
        if arguments is not None:
            msg["arguments"] = arguments
        body = json.dumps(msg).encode()
        self.proc.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.proc.stdin.flush()
        return self.seq

    def read(self):
        headers = {}
        while True:
            line = self.proc.stdout.readline().decode()
            if line in ("\r\n", "\n", ""):
                break
            key, _, value = line.partition(":")
            headers[key.strip().lower()] = value.strip()
        if "content-length" not in headers:
            raise EOFError("adapter closed its stdout")
        return json.loads(self.proc.stdout.read(int(headers["content-length"])))

    def response(self, request_seq):
        while True:
            msg = self.read()
            if msg.get("type") == "event":
                self.events.append(msg)
                continue
            if msg.get("type") == "response" and msg.get("request_seq") == request_seq:
                return msg

    def wait_event(self, name, max_msgs=200):
        for event in self.events:
            if event["event"] == name:
                return event
        for _ in range(max_msgs):
            msg = self.read()
            if msg.get("type") == "event":
                self.events.append(msg)
                if msg["event"] == name:
                    return msg
        return None

    def outputs(self):
        return "".join(
            e["body"].get("output", "") for e in self.events if e["event"] == "output"
        )

    def finish(self):
        rid = self.send("disconnect", {})
        resp = self.response(rid)
        self.proc.wait(timeout=10)
        return resp


# --- scenario 1: full session — initialize, breakpoints, launch with warp
dap = Dap()
rid = dap.send("initialize", {"adapterID": "renpy", "linesStartAt1": True})
resp = dap.response(rid)
check(
    "initialize succeeds with capabilities",
    resp.get("success") is True
    and resp.get("body", {}).get("supportsConfigurationDoneRequest") is True,
    json.dumps(resp),
)
check("initialized event arrives", dap.wait_event("initialized") is not None)

rid = dap.send("setBreakpoints", {
    "source": {"path": str(FIXTURE / "game" / "script.rpy")},
    "breakpoints": [{"line": 3}, {"line": 5}],
})
resp = dap.response(rid)
bps = resp.get("body", {}).get("breakpoints", [])
check(
    "breakpoints acknowledged unverified",
    resp.get("success") is True and len(bps) == 2
    and all(b.get("verified") is False for b in bps),
    json.dumps(resp),
)

rid = dap.send("configurationDone")
check("configurationDone accepted", dap.response(rid).get("success") is True)

warp_abs = f"{(FIXTURE / 'game' / 'script.rpy').resolve()}:5"
rid = dap.send("launch", {
    "sdk": str(FAKE_SDK.resolve()),
    "project": str(FIXTURE.resolve()),
    "warp": warp_abs,
    "env": {"FAKE_RENPY_EXIT": "0"},
})
resp = dap.response(rid)
check("launch succeeds", resp.get("success") is True, json.dumps(resp))
exited = dap.wait_event("exited")
check(
    "exited event with code 0",
    (exited or {}).get("body", {}).get("exitCode") == 0,
    json.dumps(exited),
)
check("terminated event after exit", dap.wait_event("terminated") is not None)

out = dap.outputs()
check(
    "game invoked with project + run command",
    f"FAKE-RENPY ARGS: {FIXTURE.resolve()} run" in out,
    out,
)
check("absolute warp normalized to project-relative", "--warp game/script.rpy:5" in out, out)
check("game stdout streamed to debug console", "fake renpy stdout line" in out)
check(
    "game stderr streamed with stderr category",
    any(
        e["event"] == "output"
        and e["body"].get("category") == "stderr"
        and "fake renpy stderr line" in e["body"].get("output", "")
        for e in dap.events
    ),
)

rid = dap.send("threads")
resp = dap.response(rid)
check(
    "threads lists the Ren'Py pseudo-thread",
    resp.get("body", {}).get("threads", [{}])[0].get("name") == "Ren'Py",
    json.dumps(resp),
)

check("disconnect responds", dap.finish().get("success") is True)
check("adapter exits after disconnect", dap.proc.returncode == 0, str(dap.proc.returncode))

# --- scenario 2: terminate stops a long-running game
dap = Dap()
rid = dap.send("initialize", {})
dap.response(rid)
rid = dap.send("launch", {
    "sdk": str(FAKE_SDK.resolve()),
    "project": str(FIXTURE.resolve()),
    "env": {"FAKE_RENPY_SLEEP": "30"},
})
check("long-running launch succeeds", dap.response(rid).get("success") is True)
time.sleep(0.5)  # let the fake game reach its sleep
started = time.time()
rid = dap.send("terminate", {})
check("terminate accepted", dap.response(rid).get("success") is True)
check("terminated event after kill", dap.wait_event("terminated") is not None)
check("kill was prompt, not the 30s sleep", time.time() - started < 10)
dap.finish()

# --- scenario 3: helpful error when no SDK can be found
empty_home = tempfile.mkdtemp(prefix="renpy-dap-e2e-")
env = {k: v for k, v in os.environ.items() if k != "RENPY_SDK"}
env["HOME"] = empty_home
env["USERPROFILE"] = empty_home
dap = Dap(env=env)
rid = dap.send("initialize", {})
dap.response(rid)
rid = dap.send("launch", {"project": str(FIXTURE.resolve())})
resp = dap.response(rid)
check(
    "launch without any SDK fails with guidance",
    resp.get("success") is False and "No Ren'Py SDK found" in resp.get("message", ""),
    json.dumps(resp),
)
rid = dap.send("launch", {"sdk": "/definitely/not/a/real/sdk", "project": str(FIXTURE.resolve())})
resp = dap.response(rid)
check(
    "launch with a bad SDK path names the problem",
    resp.get("success") is False
    and "does not look like a Ren'Py SDK" in resp.get("message", ""),
    json.dumps(resp),
)
dap.finish()
check("adapter exits cleanly", dap.proc.returncode == 0, str(dap.proc.returncode))

if failures:
    print(f"\n{len(failures)} FAILURE(S)")
    sys.exit(1)
print("\nALL CHECKS PASSED")
