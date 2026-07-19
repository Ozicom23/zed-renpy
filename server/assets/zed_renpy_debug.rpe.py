# Ren'Py debug agent for the zed-renpy extension.
#
# This file is written into the project's game/ directory by the extension's
# debug adapter when a debug session launches, and is safe to delete at any
# time. Ren'Py never ships .rpe* files in distributions. Without the
# ZED_RENPY_DEBUG_PORT environment variable (set only by the debug adapter)
# it does nothing at all.
#
# Design: a background thread owns a loopback socket to the debug adapter and
# speaks newline-delimited JSON. Execution hooks run on the main thread:
#   - a config.statement_callbacks entry pauses on Ren'Py statements
#     (dialogue, jump, menu, ...) whose file:line has a breakpoint, and
#     drives statement-level stepping;
#   - a sys.settrace tracer (armed only while python breakpoints exist or a
#     step is in flight) pauses inside python: blocks, $ lines and defines.
# While paused, the main thread serves stack/scopes/variables/evaluate
# requests itself — it owns the frames — until a resume command arrives.

import json
import os
import socket
import sys
import threading

PORT = os.environ.get("ZED_RENPY_DEBUG_PORT")
TOKEN = os.environ.get("ZED_RENPY_DEBUG_TOKEN", "")

RPY_SUFFIXES = (".rpy", ".rpym")


class Agent(object):
    def __init__(self):
        self.sock = None
        self.wfile = None
        self.send_lock = threading.Lock()
        self.state_lock = threading.Lock()
        # (elided_file, line) -> True, split by domain checked cheaply.
        self.breakpoints = {}  # file -> set(lines)
        self.py_files = set()  # files that currently carry breakpoints
        self.pause_requested = False
        # Stepping state (bdb-style). mode: None | "step" | "next" | "out"
        # over both domains; frames/depths recorded at the pause we left.
        self.step_mode = None
        self.step_frame = None
        self.step_stmt_depth = None
        self.stepping_python = False  # last pause was in a python frame
        # Command queue consumed by the paused main thread.
        self.commands = []
        self.commands_ready = threading.Event()
        self.paused = False
        self.configured = threading.Event()
        # Variable reference registry, valid for one pause. ref_targets
        # maps a ref to the object writes should go to (None = read-only
        # view, e.g. a function frame's locals snapshot on Python < 3.13).
        self.refs = {}
        self.ref_targets = {}
        self.next_ref = 1
        # A stable bound-method reference: sys.gettrace() identity checks fail
        # against `self.trace` because attribute access creates a new bound
        # method each time.
        self.trace_fn = self.trace

    # ---- wire ----------------------------------------------------------

    def send(self, payload):
        try:
            data = json.dumps(payload) + "\n"
            with self.send_lock:
                self.wfile.write(data.encode("utf-8"))
                self.wfile.flush()
        except Exception:
            pass

    def reader(self):
        rfile = self.sock.makefile("rb")
        while True:
            line = rfile.readline()
            if not line:
                break
            try:
                msg = json.loads(line.decode("utf-8"))
            except Exception:
                continue
            self.dispatch(msg)
        # Adapter went away: never leave the game frozen or hooked.
        with self.state_lock:
            self.breakpoints = {}
            self.py_files = set()
            self.step_mode = None
        if self.paused:
            self.enqueue({"cmd": "continue"})

    def dispatch(self, msg):
        cmd = msg.get("cmd")
        if cmd == "set_breakpoints":
            with self.state_lock:
                lines = set(msg.get("lines") or [])
                f = msg.get("file")
                if lines:
                    self.breakpoints[f] = lines
                    self.py_files.add(f)
                else:
                    self.breakpoints.pop(f, None)
                    self.py_files.discard(f)
            self.reply(msg, {"ok": True})
        elif cmd == "configuration_done":
            self.configured.set()
            self.reply(msg, {"ok": True})
        elif cmd == "pause":
            self.pause_requested = True
            self.reply(msg, {"ok": True})
        elif cmd in ("continue", "next", "step_in", "step_out", "stack",
                     "scopes", "variables", "evaluate", "set_variable"):
            if self.paused:
                self.enqueue(msg)
            elif cmd in ("stack", "scopes", "variables", "evaluate", "set_variable"):
                self.reply(msg, {"error": "not stopped"})
            else:
                self.reply(msg, {"ok": True})
        else:
            self.reply(msg, {"error": "unknown command"})

    def reply(self, msg, payload):
        if "id" in msg:
            payload = dict(payload)
            payload["reply_to"] = msg["id"]
            self.send(payload)

    def enqueue(self, msg):
        self.commands.append(msg)
        self.commands_ready.set()

    # ---- breakpoint checks (hot paths) ---------------------------------

    def stmt_callback(self, name):
        # Called before every Ren'Py statement, on the main thread.
        self.sync_trace()
        if self.paused:
            return
        why = None
        if self.pause_requested:
            why = "pause"
        elif self.step_mode is not None:
            if self.stepping_python:
                # A statement boundary means the python block we were
                # stepping through has finished — the step completes here.
                why = "step"
            elif self.step_mode == "step":
                why = "step"
            else:
                import renpy
                depth = renpy.exports.call_stack_depth()
                if self.step_mode == "next" and depth <= (self.step_stmt_depth or 0):
                    why = "step"
                elif self.step_mode == "out" and depth < (self.step_stmt_depth or 0):
                    why = "step"
        if why is None and self.breakpoints:
            import renpy
            f, line = renpy.exports.get_filename_line()
            lines = self.breakpoints.get(normalize(f))
            if lines and line in lines:
                why = "breakpoint"
        if why is not None:
            self.pause(why, frame=None)

    def trace(self, frame, event, arg):
        if event != "call":
            return None
        if not (self.py_files or self.step_mode):
            return None
        fn = frame.f_code.co_filename
        if not fn.endswith(RPY_SUFFIXES):
            return None
        return self.trace_local

    def trace_local(self, frame, event, arg):
        if self.paused:
            return self.trace_local
        if event == "line":
            why = None
            if self.pause_requested:
                why = "pause"
            elif self.step_mode == "step":
                why = "step"
            elif (self.step_mode == "next" and self.stepping_python
                  and (frame is self.step_frame or self._in_caller_chain(frame))):
                why = "step"
            elif self.step_mode == "out" and self.stepping_python and self._in_caller_chain(frame):
                why = "step"
            if why is None:
                lines = self.breakpoints.get(normalize(frame.f_code.co_filename))
                if lines and frame.f_lineno in lines:
                    why = "breakpoint"
            # The current statement's own line belongs to the statement
            # callback (which fires around it and stops in the statement
            # domain). Ren'Py also evaluates tiny per-statement expressions
            # attributed to that same file:line before the statement runs;
            # stopping on those would pause at a ghost python frame.
            if why is not None and self._is_current_statement_line(frame):
                why = None
            if why is not None:
                self.pause(why, frame=frame)
        return self.trace_local

    def _is_current_statement_line(self, frame):
        try:
            import renpy
            f, line = renpy.exports.get_filename_line()
            return line == frame.f_lineno and normalize(f) == normalize(frame.f_code.co_filename)
        except Exception:
            return False

    def _in_caller_chain(self, frame):
        # True if `frame` is a caller (strict ancestor) of the frame we
        # stepped from — i.e. that frame has returned.
        f = self.step_frame.f_back if self.step_frame is not None else None
        while f is not None:
            if f is frame:
                return True
            f = f.f_back
        return False

    def sync_trace(self):
        # Arm/disarm the (costly) python tracer from the main thread, so the
        # game pays nothing while no python breakpoints or steps are active.
        want = bool(self.py_files) or self.step_mode is not None
        if want and sys.gettrace() is not self.trace_fn:
            sys.settrace(self.trace_fn)
        elif not want and sys.gettrace() is self.trace_fn:
            sys.settrace(None)

    # ---- pausing -------------------------------------------------------

    def pause(self, reason, frame):
        self.paused = True
        self.pause_requested = False
        self.step_mode = None
        self.refs = {}
        self.ref_targets = {}
        self.next_ref = 1
        stack = self.build_stack(frame)
        top = stack[0] if stack else {"file": "unknown", "line": 0}
        self.send({
            "event": "stopped",
            "reason": reason,
            "file": top.get("file"),
            "line": top.get("line"),
        })
        self._stack = stack
        self._frame = frame
        while True:
            if not self.commands:
                self.commands_ready.wait()
            self.commands_ready.clear()
            resume = None
            while self.commands:
                msg = self.commands.pop(0)
                cmd = msg.get("cmd")
                if cmd == "stack":
                    self.reply(msg, {"frames": self._stack})
                elif cmd == "scopes":
                    self.reply(msg, {"scopes": self.build_scopes(msg.get("frame") or 0)})
                elif cmd == "variables":
                    self.reply(msg, {"variables": self.expand_ref(msg.get("ref") or 0)})
                elif cmd == "evaluate":
                    self.reply(msg, self.evaluate(msg.get("expr") or ""))
                elif cmd == "set_variable":
                    self.reply(msg, self.set_variable(
                        msg.get("ref") or 0, msg.get("name") or "",
                        msg.get("value") or "None"))
                elif cmd in ("continue", "next", "step_in", "step_out"):
                    self.reply(msg, {"ok": True})
                    resume = cmd
                    break
            if resume is not None:
                break
        self.apply_resume(resume, frame)
        self._stack = None
        self._frame = None
        self.paused = False
        self.sync_trace()

    def apply_resume(self, cmd, frame):
        self.stepping_python = frame is not None
        if cmd == "continue":
            self.step_mode = None
        elif cmd == "step_in":
            self.step_mode = "step"
        elif cmd == "next":
            self.step_mode = "next"
        elif cmd == "step_out":
            self.step_mode = "out"
        self.step_frame = frame
        if self.step_mode in ("next", "out") and frame is None:
            try:
                import renpy
                self.step_stmt_depth = renpy.exports.call_stack_depth()
            except Exception:
                self.step_stmt_depth = 0

    # ---- inspection ----------------------------------------------------

    def build_stack(self, frame):
        frames = []
        if frame is not None:
            f = frame
            while f is not None:
                fn = f.f_code.co_filename
                if fn.endswith(RPY_SUFFIXES):
                    frames.append({
                        "name": f.f_code.co_name if f.f_code.co_name != "<module>" else "python block",
                        "file": normalize(fn),
                        "line": f.f_lineno,
                        "kind": "python",
                        "frame": len(frames),
                    })
                f = f.f_back
        try:
            import renpy
            fn, line = renpy.exports.get_filename_line()
            frames.append({
                "name": "statement: " + str(renpy.ast.current_statement_name),
                "file": normalize(fn),
                "line": line,
                "kind": "statement",
                "frame": len(frames),
            })
            namemap = renpy.game.script.namemap
            for entry in reversed(renpy.exports.get_return_stack()):
                node = namemap.get(entry)
                if node is not None:
                    frames.append({
                        "name": "call from " + label_name(node),
                        "file": normalize(node.filename),
                        "line": node.linenumber,
                        "kind": "statement",
                        "frame": len(frames),
                    })
        except Exception:
            pass
        # Frame indexes double as DAP ids; keep a lookup for scope building.
        self._py_frames = []
        f = frame
        while f is not None:
            if f.f_code.co_filename.endswith(RPY_SUFFIXES):
                self._py_frames.append(f)
            f = f.f_back
        return frames

    def build_scopes(self, frame_index):
        scopes = []
        py_frames = getattr(self, "_py_frames", [])
        if frame_index < len(py_frames):
            f = py_frames[frame_index]
            if f.f_code.co_name == "<module>":
                # Module-style frames (init python, python:, $) expose their
                # real namespace — edits take effect.
                scopes.append({"name": "Locals", "ref": self.register(f.f_locals)})
            else:
                # CPython < 3.13: function locals are snapshots; show them
                # but refuse writes rather than silently dropping them.
                scopes.append({
                    "name": "Locals",
                    "ref": self.register(dict(f.f_locals), target=None),
                })
        try:
            import renpy
            store = renpy.python.store_dicts["store"]
            visible = {}
            for key in list(store.keys()):
                if key.startswith("_"):
                    continue
                value = store[key]
                if type(value).__name__ in ("module", "function", "type", "builtin_function_or_method"):
                    continue
                visible[key] = value
            scopes.append({"name": "Store", "ref": self.register(visible, target=store)})
        except Exception:
            pass
        return scopes

    def register(self, obj, target="same"):
        ref = self.next_ref
        self.next_ref += 1
        self.refs[ref] = obj
        self.ref_targets[ref] = obj if target == "same" else target
        return ref

    def expand_ref(self, ref):
        obj = self.refs.get(ref)
        if obj is None:
            return []
        out = []
        try:
            if isinstance(obj, dict):
                items = sorted(obj.items(), key=lambda kv: str(kv[0]))[:1000]
                for key, value in items:
                    out.append(self.variable(str(key), value))
            elif isinstance(obj, (list, tuple)):
                for i, value in enumerate(list(obj)[:1000]):
                    out.append(self.variable("[%d]" % i, value))
            else:
                for key in sorted(dir(obj))[:200]:
                    if key.startswith("_"):
                        continue
                    try:
                        value = getattr(obj, key)
                    except Exception:
                        continue
                    if callable(value):
                        continue
                    out.append(self.variable(key, value))
                    if len(out) >= 100:
                        break
        except Exception:
            pass
        return out

    def variable(self, name, value):
        ref = 0
        if isinstance(value, (dict, list, tuple)) and value:
            ref = self.register(value)
        elif hasattr(value, "__dict__") and not isinstance(value, type):
            ref = self.register(value)
        return {
            "name": name,
            "value": safe_repr(value),
            "type": type(value).__name__,
            "ref": ref,
        }

    def _eval(self, expr):
        py_frames = getattr(self, "_py_frames", [])
        if py_frames:
            f = py_frames[0]
            return eval(expr, f.f_globals, f.f_locals)
        import renpy
        return renpy.python.py_eval(expr)

    def evaluate(self, expr):
        try:
            try:
                value = self._eval(expr)
            except SyntaxError:
                # Not an expression — execute it as a statement (assignments,
                # calls with side effects) in the same scope.
                py_frames = getattr(self, "_py_frames", [])
                if py_frames:
                    f = py_frames[0]
                    exec(expr, f.f_globals, f.f_locals)
                else:
                    import renpy
                    renpy.python.py_exec(expr)
                return {"value": "(executed)", "type": "", "ref": 0}
            result = self.variable("result", value)
            return {"value": result["value"], "type": result["type"], "ref": result["ref"]}
        except Exception as e:
            return {"error": "%s: %s" % (type(e).__name__, e)}

    def set_variable(self, ref, name, value_expr):
        target = self.ref_targets.get(ref)
        if target is None:
            return {"error": "this scope is read-only here (function locals "
                             "cannot be modified on this Python version)"}
        try:
            value = self._eval(value_expr)
        except Exception as e:
            return {"error": "%s: %s" % (type(e).__name__, e)}
        try:
            if isinstance(target, dict):
                target[name] = value
            elif isinstance(target, list):
                index = int(name.strip("[]"))
                target[index] = value
            elif isinstance(target, tuple):
                return {"error": "tuples are immutable"}
            else:
                setattr(target, name, value)
        except Exception as e:
            return {"error": "%s: %s" % (type(e).__name__, e)}
        # Keep the displayed view in sync when it is a separate copy.
        display = self.refs.get(ref)
        if isinstance(display, dict) and display is not target:
            display[name] = value
        return {"variable": self.variable(name, value)}


def normalize(filename):
    return filename.replace("\\", "/")


def label_name(node):
    name = getattr(node, "name", None)
    if isinstance(name, str):
        return name
    return "%s:%d" % (normalize(node.filename), node.linenumber)


def safe_repr(value, limit=200):
    try:
        r = repr(value)
    except Exception:
        try:
            r = "<unreprable %s>" % type(value).__name__
        except Exception:
            r = "<unreprable>"
    if len(r) > limit:
        r = r[:limit] + "…"
    return r


def start():
    # Ren'Py can load the same .rpe.py from more than one search path;
    # registering the hooks twice would double every pause.
    if getattr(sys, "_zed_renpy_debug_agent", None) is not None:
        return
    agent = Agent()
    sys._zed_renpy_debug_agent = agent
    try:
        sock = socket.create_connection(("127.0.0.1", int(PORT)), timeout=5)
    except Exception:
        return
    sock.settimeout(None)
    agent.sock = sock
    agent.wfile = sock.makefile("wb")
    agent.send({"event": "hello", "token": TOKEN, "version": 1})

    reader = threading.Thread(target=agent.reader, name="zed-renpy-debug", daemon=True)
    reader.start()

    # Let the adapter deliver the initial breakpoints before any init python
    # runs, so startup-time breakpoints bind deterministically.
    agent.configured.wait(5)

    import renpy.config
    renpy.config.statement_callbacks.append(agent.stmt_callback)
    agent.sync_trace()


if PORT:
    try:
        start()
    except Exception:
        pass
