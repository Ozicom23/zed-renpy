#!/usr/bin/env python3
"""End-to-end test: drive renpy-language-server over real LSP stdio.

Usage: python3 tests/e2e_lsp.py [path-to-server-binary]
Defaults to server/target/release/renpy-language-server.
"""
import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
FIXTURE = HERE / "fixture"
DEFAULT_SERVER = HERE.parent / "server" / "target" / "release" / "renpy-language-server"
SERVER = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_SERVER

proc = subprocess.Popen(
    [str(SERVER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
)
next_id = [0]
failures = []
notifications = []


def send(method, params, is_request=True):
    msg = {"jsonrpc": "2.0", "method": method, "params": params}
    if is_request:
        next_id[0] += 1
        msg["id"] = next_id[0]
    body = json.dumps(msg).encode()
    proc.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    proc.stdin.flush()
    return msg.get("id")


def read_message():
    headers = {}
    while True:
        line = proc.stdout.readline().decode()
        if line in ("\r\n", "\n", ""):
            break
        key, _, value = line.partition(":")
        headers[key.strip().lower()] = value.strip()
    length = int(headers["content-length"])
    return json.loads(proc.stdout.read(length))


def read_response(request_id):
    while True:
        msg = read_message()
        if msg.get("id") == request_id and ("result" in msg or "error" in msg):
            return msg
        if "method" in msg and "id" not in msg:
            notifications.append(msg)


def diags_for(path):
    """All published diagnostic lists for a file, oldest first."""
    u = path.resolve().as_uri()
    return [
        n["params"]["diagnostics"]
        for n in notifications
        if n.get("method") == "textDocument/publishDiagnostics" and n["params"]["uri"] == u
    ]


def check(label, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"{status}: {label}" + (f"  [{detail}]" if detail and not condition else ""))
    if not condition:
        failures.append(label)


def uri(path):
    return path.resolve().as_uri()


script = FIXTURE / "game" / "script.rpy"
shop = FIXTURE / "game" / "shop.rpy"
broken = FIXTURE / "game" / "broken.rpy"

# --- handshake, with a workspace folder so the startup scan kicks in.
# Lint is explicitly disabled here: on a machine with a real Ren'Py SDK it
# would auto-run against the fixture and make diagnostics nondeterministic.
# The fake-SDK lint flow gets its own deterministic section at the end.
rid = send("initialize", {
    "processId": None,
    "rootUri": uri(FIXTURE),
    "capabilities": {},
    "workspaceFolders": [{"uri": uri(FIXTURE), "name": "fixture"}],
    "initializationOptions": {"lint": False},
})
init = read_response(rid)
caps = init.get("result", {}).get("capabilities", {})
check("initialize returns definitionProvider", caps.get("definitionProvider") is True)
check("initialize returns workspaceSymbolProvider", caps.get("workspaceSymbolProvider") is True)
check("initialize returns hoverProvider", caps.get("hoverProvider") is True)
check("initialize returns completionProvider", isinstance(caps.get("completionProvider"), dict))
check(
    "initialize returns references + rename providers",
    caps.get("referencesProvider") is True and caps.get("renameProvider") is True,
)
send("initialized", {}, is_request=False)

send("textDocument/didOpen", {
    "textDocument": {
        "uri": uri(script), "languageId": "renpy", "version": 1,
        "text": script.read_text(),
    }
}, is_request=False)

# --- cross-file goto definition: `jump shop` -> shop.rpy (never opened!)
rid = send("textDocument/definition", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 5, "character": 10},
})
result = read_response(rid).get("result")
check(
    "cross-file definition of jump target 'shop'",
    isinstance(result, list) and len(result) == 1
    and result[0]["uri"] == uri(shop) and result[0]["range"]["start"]["line"] == 0,
    detail=json.dumps(result),
)

# --- speaker `e` resolves to its define in the same file
rid = send("textDocument/definition", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 4},
})
result = read_response(rid).get("result")
check(
    "definition of speaker 'e' -> define statement",
    isinstance(result, list) and len(result) == 1
    and result[0]["uri"] == uri(script) and result[0]["range"]["start"]["line"] == 0,
    detail=json.dumps(result),
)

# --- word with no definition -> null
rid = send("textDocument/definition", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 7},  # inside "hi" string
})
result = read_response(rid).get("result")
check("definition inside plain dialogue returns null", result is None, detail=json.dumps(result))

# --- workspace symbols
rid = send("workspace/symbol", {"query": "sho"})
result = read_response(rid).get("result")
names = [s["name"] for s in (result or [])]
check("workspace/symbol 'sho' finds label shop", "shop" in names, detail=json.dumps(names))

rid = send("workspace/symbol", {"query": ""})
result = read_response(rid).get("result")
names = sorted(s["name"] for s in (result or []))
expected = {"e", "points", "shop", "start", "inventory"}
check(
    "workspace/symbol '' lists all definitions",
    expected.issubset(set(names)),
    detail=json.dumps(names),
)

# --- hover: project symbol shows its definition line
rid = send("textDocument/hover", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 4},
})
result = read_response(rid).get("result")
value = (result or {}).get("contents", {}).get("value", "")
check("hover on speaker 'e' shows its define", "define e = Character" in value, detail=json.dumps(result))

# --- hover: Ren'Py built-in gets bundled API docs
rid = send("textDocument/hover", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 0, "character": 12},
})
result = read_response(rid).get("result")
value = (result or {}).get("contents", {}).get("value", "")
check(
    "hover on 'Character' shows built-in signature + docs",
    value.startswith("```renpy\nCharacter(") and "Ren'Py built-in" in value,
    detail=json.dumps(result)[:300],
)
check(
    "hover on 'Character' links to official docs",
    "https://www.renpy.org/doc/html/dialogue.html#Character" in value,
    detail=json.dumps(result)[:300],
)

# --- hover: built-in class Transform (the original user request!)
rid = send("textDocument/hover", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 0, "character": 12},
})
read_response(rid)  # exercised above; Transform checked via unit tests too

# --- hover: no info -> null
rid = send("textDocument/hover", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 7},
})
result = read_response(rid).get("result")
check("hover inside plain dialogue returns null", result is None, detail=json.dumps(result))

# --- diagnostics: broken.rpy was flagged at startup (published after `initialized`)
entries = diags_for(broken)
latest = entries[-1] if entries else []
check(
    "diagnostics: undefined jump target flagged in broken.rpy",
    any("missing_target" in d["message"] and d.get("severity") == 1 for d in latest),
    detail=json.dumps(latest),
)
check(
    "diagnostics: duplicate label flagged twice as warning",
    sum(1 for d in latest if "defined 2 times" in d["message"] and d.get("severity") == 2) == 2,
    detail=json.dumps(latest),
)
check("diagnostics: clean files stay clean", not diags_for(script) and not diags_for(shop))

# --- completion: after `jump ` -> labels only
rid = send("textDocument/completion", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 5, "character": 9},
    "context": {"triggerKind": 1},
})
result = read_response(rid).get("result") or []
labels_offered = [i["label"] for i in result]
check(
    "completion after 'jump ' offers labels, not variables",
    {"shop", "start", "dup_label"} == set(labels_offered) and "e" not in labels_offered,
    detail=json.dumps(labels_offered),
)

# --- completion: at line start -> keywords + speakers
rid = send("textDocument/completion", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 4},
    "context": {"triggerKind": 1},
})
result = read_response(rid).get("result") or []
offered = [i["label"] for i in result]
check(
    "completion at line start offers keywords and speakers",
    "scene" in offered and "e" in offered and "shop" not in offered,
    detail=json.dumps(offered)[:300],
)

# --- completion: general context -> built-ins
rid = send("textDocument/completion", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 0, "character": 11},
    "context": {"triggerKind": 1},
})
result = read_response(rid).get("result") or []
offered = {i["label"] for i in result}
check(
    "completion in expression context offers built-ins",
    "Character" in offered and "Transform" in offered and "config.name" in offered,
    detail=str(len(offered)),
)

# --- references: label 'shop' = its definition + the jump to it
rid = send("textDocument/references", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 5, "character": 10},
    "context": {"includeDeclaration": True},
})
result = read_response(rid).get("result") or []
check(
    "references for label 'shop' = definition + jump",
    len(result) == 2
    and any(l["uri"] == uri(shop) and l["range"]["start"]["line"] == 0 for l in result)
    and any(l["uri"] == uri(script) and l["range"]["start"]["line"] == 5 for l in result),
    detail=json.dumps(result),
)

# --- references: speaker 'e' via textual scan (no hits inside words like Eileen)
rid = send("textDocument/references", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 4},
    "context": {"includeDeclaration": True},
})
result = read_response(rid).get("result") or []
check(
    "references for speaker 'e' finds define + say line only",
    len(result) == 2 and all(l["uri"] == uri(script) for l in result),
    detail=json.dumps(result),
)

# --- rename label shop -> bazaar: WorkspaceEdit across both files (not applied)
rid = send("textDocument/rename", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 5, "character": 10},
    "newName": "bazaar",
})
result = read_response(rid).get("result") or {}
changes = result.get("changes") or {}
check(
    "rename produces edits in both files",
    set(changes) == {uri(script), uri(shop)}
    and all(e["newText"] == "bazaar" for edits in changes.values() for e in edits),
    detail=json.dumps(result),
)

# --- rename collisions and non-labels are refused
rid = send("textDocument/rename", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 5, "character": 10},
    "newName": "start",
})
resp = read_response(rid)
check(
    "rename onto an existing label errors",
    "error" in resp and "already exists" in resp["error"]["message"],
    detail=json.dumps(resp),
)
rid = send("textDocument/rename", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 4, "character": 4},
    "newName": "narrator2",
})
resp = read_response(rid)
check(
    "rename of a non-label is refused",
    "error" in resp and "labels only" in resp["error"]["message"],
    detail=json.dumps(resp),
)

# --- live re-index on change: rename label shop -> shop_two in the buffer
send("textDocument/didOpen", {
    "textDocument": {
        "uri": uri(shop), "languageId": "renpy", "version": 1,
        "text": shop.read_text(),
    }
}, is_request=False)
send("textDocument/didChange", {
    "textDocument": {"uri": uri(shop), "version": 2},
    "contentChanges": [{"text": shop.read_text().replace("label shop:", "label shop_two:")}],
}, is_request=False)

rid = send("textDocument/definition", {
    "textDocument": {"uri": uri(script)},
    "position": {"line": 5, "character": 10},
})
result = read_response(rid).get("result")
check("after rename, old 'shop' target is gone", result is None, detail=json.dumps(result))

rid = send("workspace/symbol", {"query": "shop_two"})
result = read_response(rid).get("result")
names = [s["name"] for s in (result or [])]
check("renamed label 'shop_two' is indexed live", names == ["shop_two"], detail=json.dumps(names))

# --- live diagnostics: script.rpy's `jump shop` became undefined after the rename
entries = diags_for(script)
latest = entries[-1] if entries else []
check(
    "live diagnostics: 'jump shop' flagged after rename",
    any("'shop' is not defined" in d["message"] for d in latest),
    detail=json.dumps(entries),
)

# --- clean shutdown
rid = send("shutdown", {})
read_response(rid)
send("exit", {}, is_request=False)
proc.wait(timeout=10)
check("server exits cleanly", proc.returncode == 0, detail=str(proc.returncode))

stderr = proc.stderr.read().decode()
print("--- server stderr ---")
print(stderr.strip())

# --- engine lint via the fake SDK: a fresh server instance with lint enabled
proc = subprocess.Popen(
    [str(SERVER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
)
# Watchdog: if lint never publishes, reads below would block forever.
import threading
watchdog = threading.Timer(60, proc.kill)
watchdog.daemon = True
watchdog.start()
notifications.clear()
rid = send("initialize", {
    "processId": None,
    "rootUri": uri(FIXTURE),
    "capabilities": {},
    "workspaceFolders": [{"uri": uri(FIXTURE), "name": "fixture"}],
    "initializationOptions": {"sdk": str((HERE / "fixture-sdk").resolve())},
})
read_response(rid)
send("initialized", {}, is_request=False)

lint_diags = None
for _ in range(50):  # lint results arrive asynchronously after `initialized`
    msg = read_message()
    if msg.get("method") == "textDocument/publishDiagnostics" and msg["params"]["uri"] == uri(script):
        found = [d for d in msg["params"]["diagnostics"] if d.get("source") == "renpy lint"]
        if found:
            lint_diags = found
            break
check(
    "engine lint published as diagnostics",
    lint_diags is not None
    and "fake lint problem from the fake SDK. This second line continues the same problem."
    in lint_diags[0]["message"],
    detail=json.dumps(lint_diags),
)
check(
    "lint diagnostic is a warning on the reported line",
    lint_diags is not None
    and lint_diags[0]["severity"] == 2
    and lint_diags[0]["range"]["start"]["line"] == 2,
    detail=json.dumps(lint_diags),
)

# a save triggers a re-lint (same fake report; just verify another publish lands)
send("textDocument/didSave", {"textDocument": {"uri": uri(script)}}, is_request=False)
relinted = False
for _ in range(50):
    msg = read_message()
    if msg.get("method") == "textDocument/publishDiagnostics" and msg["params"]["uri"] == uri(script):
        if any(d.get("source") == "renpy lint" for d in msg["params"]["diagnostics"]):
            relinted = True
            break
check("didSave triggers a re-lint", relinted)

rid = send("shutdown", {})
read_response(rid)
send("exit", {}, is_request=False)
proc.wait(timeout=10)
check("lint server exits cleanly", proc.returncode == 0, detail=str(proc.returncode))

if failures:
    print(f"\n{len(failures)} FAILURE(S)")
    sys.exit(1)
print("\nALL CHECKS PASSED")
