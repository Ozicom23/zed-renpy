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
#   - a sys.settrace tracer (armed only while python breakpoints exist, a
#     step is in flight or "all raised exceptions" is on) pauses inside
#     python: blocks, $ lines and defines;
#   - a wrapper around renpy.error.report_exception pauses on script errors
#     before Ren'Py shows its traceback screen, with the failing frames
#     still inspectable.
# Breakpoints may carry a condition, a hit condition and a log message
# (a logpoint prints instead of pausing).
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
        # file -> {line: {"condition", "hit", "log", "hits"}}
        self.breakpoints = {}
        self.py_files = set()  # files that currently carry breakpoints
        self.pause_requested = False
        # Exception filters chosen in the editor: "uncaught" (script errors
        # Ren'Py would report) and "raised" (any exception in game python).
        self.exc_uncaught = False
        self.exc_raised = False
        # The exception the current pause is about, for exception_info, and
        # the last one paused on, so one error never pauses twice.
        self.exception = None
        self.last_exception = None
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
        # Set when a hook raised: every hook then stands down for good, so a
        # bug here (or an engine API change) degrades to "no breakpoints"
        # instead of surfacing as a game exception on every statement.
        self.disabled = False
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
            specs = {}
            for bp in msg.get("breakpoints") or [{"line": l} for l in msg.get("lines") or []]:
                specs[bp.get("line")] = {
                    "condition": bp.get("condition") or None,
                    "hit": bp.get("hitCondition") or None,
                    "log": bp.get("logMessage") or None,
                    "hits": 0,
                }
            with self.state_lock:
                f = msg.get("file")
                if specs:
                    self.breakpoints[f] = specs
                    self.py_files.add(f)
                else:
                    self.breakpoints.pop(f, None)
                    self.py_files.discard(f)
            self.reply(msg, {"ok": True})
        elif cmd == "set_exception_filters":
            filters = msg.get("filters") or []
            self.exc_uncaught = "uncaught" in filters
            self.exc_raised = "raised" in filters
            self.reply(msg, {"ok": True})
        elif cmd == "configuration_done":
            self.configured.set()
            self.reply(msg, {"ok": True})
        elif cmd == "pause":
            self.pause_requested = True
            self.reply(msg, {"ok": True})
        elif cmd in ("continue", "next", "step_in", "step_out", "stack",
                     "scopes", "variables", "evaluate", "set_variable", "exception_info"):
            if self.paused:
                self.enqueue(msg)
            elif cmd in ("stack", "scopes", "variables", "evaluate", "set_variable", "exception_info"):
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
        if self.disabled:
            return
        try:
            self._stmt_callback(name)
        except Exception as e:
            self.disable("statement hook failed", e)

    def _stmt_callback(self, name):
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
            specs = self.breakpoints.get(normalize(f))
            if specs and line in specs and self.should_stop(specs[line], None):
                why = "breakpoint"
        if why is not None:
            self.pause(why, frame=None)

    def trace(self, frame, event, arg):
        if event != "call" or self.disabled:
            return None
        if not (self.py_files or self.step_mode or self.exc_raised):
            return None
        fn = frame.f_code.co_filename
        if not fn.endswith(RPY_SUFFIXES):
            return None
        return self.trace_local

    def trace_local(self, frame, event, arg):
        # An exception escaping a trace function both unhooks the tracer and
        # propagates into the traced code; contain it instead.
        if self.disabled:
            return None
        try:
            return self._trace_local(frame, event, arg)
        except Exception as e:
            self.disable("python trace hook failed", e)
            return None

    def _trace_local(self, frame, event, arg):
        if self.paused:
            return self.trace_local
        if event == "exception":
            # Ren'Py's own scripts (renpy/common) raise and catch exceptions
            # as a matter of course; only the game's code is of interest.
            if (self.exc_raised and is_reportable(arg[1]) and arg[1] is not self.last_exception
                    and not normalize(frame.f_code.co_filename).startswith("renpy/common/")):
                self.pause("exception", frame=frame, exception=arg[1], mode="always")
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
            breakpoint_spec = None
            if why is None:
                specs = self.breakpoints.get(normalize(frame.f_code.co_filename))
                if specs and frame.f_lineno in specs:
                    breakpoint_spec = specs[frame.f_lineno]
                    why = "breakpoint"
            # The current statement's own line belongs to the statement
            # callback (which fires around it and stops in the statement
            # domain). Ren'Py also evaluates tiny per-statement expressions
            # attributed to that same file:line before the statement runs;
            # stopping on those would pause at a ghost python frame.
            if why is not None and self._is_current_statement_line(frame):
                why = None
            # Conditions, hit counts and logpoints are checked last, so a
            # ghost frame never counts as a hit.
            if why == "breakpoint" and not self.should_stop(breakpoint_spec, frame):
                why = None
            if why is not None:
                self.pause(why, frame=frame)
        return self.trace_local

    # ---- conditions, hit counts, logpoints ------------------------------

    def eval_in(self, expr, frame):
        if frame is not None:
            return eval(expr, frame.f_globals, frame.f_locals)
        import renpy
        return renpy.python.py_eval(expr)

    def should_stop(self, spec, frame):
        # Called when execution reaches a breakpoint's line. False means the
        # breakpoint does not fire this time (condition false, hit count not
        # reached, or a logpoint, which only prints).
        if spec["condition"]:
            try:
                if not self.eval_in(spec["condition"], frame):
                    return False
            except Exception as e:
                self.output("Breakpoint condition %r failed (%s: %s); pausing.\n"
                            % (spec["condition"], type(e).__name__, e))
                return True
        spec["hits"] += 1
        if spec["hit"] and not hit_condition_met(spec["hit"], spec["hits"]):
            return False
        if spec["log"]:
            self.output(self.interpolate(spec["log"], frame) + "\n")
            return False
        return True

    def interpolate(self, message, frame):
        # Logpoint text: {expression} is replaced by its value, {{ and }}
        # are literal braces.
        out = []
        i = 0
        while i < len(message):
            c = message[i]
            if c == "{" and message.startswith("{{", i):
                out.append("{")
                i += 2
            elif c == "}" and message.startswith("}}", i):
                out.append("}")
                i += 2
            elif c == "{":
                end = message.find("}", i)
                if end < 0:
                    out.append(message[i:])
                    break
                expr = message[i + 1:end]
                try:
                    value = self.eval_in(expr, frame)
                    out.append(value if isinstance(value, str) else safe_repr(value))
                except Exception as e:
                    out.append("<%s: %s>" % (type(e).__name__, e))
                i = end + 1
            else:
                out.append(c)
                i += 1
        return "".join(out)

    def output(self, text):
        self.send({"event": "output", "text": text})

    # ---- exceptions ----------------------------------------------------

    def report_hook(self, original):
        # Wraps renpy.error.report_exception: Ren'Py calls it for every
        # script error (at init and in game) before it shows the traceback
        # screen. The traceback still holds the failing frames.
        agent = self

        def report_exception(e, *args, **kwargs):
            try:
                if (agent.exc_uncaught and not agent.disabled and not agent.paused
                        and isinstance(e, BaseException) and e is not agent.last_exception):
                    agent.pause("exception", frame=deepest_script_frame(e),
                                exception=e, mode="unhandled")
            except Exception as hook_error:
                agent.disable("exception hook failed", hook_error)
            return original(e, *args, **kwargs)

        report_exception._zed_original = original
        return report_exception

    def exception_info(self):
        e = self.exception
        if e is None:
            return {"error": "not stopped on an exception"}
        import traceback
        try:
            trace = "".join(traceback.format_exception(type(e), e, e.__traceback__))
        except Exception:
            trace = ""
        return {
            "type": type(e).__name__,
            "message": str(e),
            "mode": getattr(self, "_exception_mode", "unhandled"),
            "traceback": trace,
        }

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

    def disable(self, what, exc):
        self.disabled = True
        self.paused = False
        self.pause_requested = False
        with self.state_lock:
            self.breakpoints = {}
            self.py_files = set()
            self.step_mode = None
        try:
            if sys.gettrace() is self.trace_fn:
                sys.settrace(None)
        except Exception:
            pass
        self.send({
            "event": "error",
            "message": "%s (%s: %s); breakpoints are inactive for the rest of this run"
                       % (what, type(exc).__name__, exc),
        })

    def sync_trace(self):
        # Arm/disarm the (costly) python tracer from the main thread, so the
        # game pays nothing while no python breakpoints or steps are active.
        want = bool(self.py_files) or self.step_mode is not None or self.exc_raised
        if want and sys.gettrace() is not self.trace_fn:
            sys.settrace(self.trace_fn)
        elif not want and sys.gettrace() is self.trace_fn:
            sys.settrace(None)

    # ---- pausing -------------------------------------------------------

    def pause(self, reason, frame, exception=None, mode=None):
        self.paused = True
        self.pause_requested = False
        self.step_mode = None
        self.refs = {}
        self.ref_targets = {}
        self.next_ref = 1
        self.exception = exception
        self._exception_mode = mode
        if exception is not None:
            self.last_exception = exception
        stack = self.build_stack(frame)
        top = stack[0] if stack else {"file": "unknown", "line": 0}
        stopped = {
            "event": "stopped",
            "reason": reason,
            "file": top.get("file"),
            "line": top.get("line"),
        }
        if exception is not None:
            stopped["description"] = "Paused on %s" % type(exception).__name__
            stopped["text"] = "%s: %s" % (type(exception).__name__, exception)
        self.send(stopped)
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
                elif cmd == "exception_info":
                    self.reply(msg, self.exception_info())
                elif cmd in ("continue", "next", "step_in", "step_out"):
                    self.reply(msg, {"ok": True})
                    resume = cmd
                    break
            if resume is not None:
                break
        # After an exception there is nothing to step into: Ren'Py unwinds
        # to its error handling, so any resume continues.
        self.apply_resume("continue" if exception is not None else resume, frame)
        self.exception = None
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


def hit_condition_met(condition, hits):
    # DAP hit conditions as editors write them: "5" or "==5" (exactly the
    # 5th hit), ">5", ">=5", "<5", "<=5", "%5" (every 5th hit).
    text = condition.replace(" ", "")
    for op, test in ((">=", lambda n: hits >= n), ("<=", lambda n: hits <= n),
                     ("==", lambda n: hits == n), (">", lambda n: hits > n),
                     ("<", lambda n: hits < n), ("%", lambda n: n > 0 and hits % n == 0)):
        if text.startswith(op):
            try:
                return test(int(text[len(op):]))
            except ValueError:
                return True
    try:
        return hits == int(text)
    except ValueError:
        return True


# Exceptions Ren'Py uses for control flow (jump, call, restart, quit, ...)
# and Python's iteration protocol: never errors.
CONTROL_FLOW = ("StopIteration", "StopAsyncIteration", "GeneratorExit")


def is_reportable(e):
    if e is None or type(e).__name__ in CONTROL_FLOW:
        return False
    module = type(e).__module__ or ""
    return not (module == "renpy" or module.startswith("renpy."))


def deepest_script_frame(e):
    # The innermost frame of the exception's traceback that runs game script
    # code, or None when the error was raised by the engine itself (the
    # pause then shows the failing statement).
    frame = None
    tb = getattr(e, "__traceback__", None)
    while tb is not None:
        if tb.tb_frame.f_code.co_filename.endswith(RPY_SUFFIXES):
            frame = tb.tb_frame
        tb = tb.tb_next
    return frame


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
    import renpy.error
    renpy.config.statement_callbacks.append(agent.stmt_callback)
    renpy.error.report_exception = agent.report_hook(renpy.error.report_exception)
    agent.sync_trace()


if PORT:
    try:
        start()
    except Exception:
        pass
