#!/usr/bin/env python3
"""Run languages/renpy/tasks.json the way Zed does, against a fake SDK.

Zed substitutes $ZED_* variables, then pastes `command` and every arg into
one command line for the user's shell WITHOUT quoting them
(ShellBuilder::build_no_quote) and runs `<shell> -i -c <line>`. A task that
forgets to quote its own words breaks in the user's shell, not in Zed, so
this replays exactly that with every POSIX shell available, including a
project path with spaces and an apostrophe.

Usage: python3 tests/test_tasks.py
"""
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).parent
TASKS = json.loads((HERE.parent / "languages" / "renpy" / "tasks.json").read_text())

failures = []


def check(label, condition, detail=""):
    print(("PASS" if condition else "FAIL") + f": {label}" + (f"  [{detail}]" if detail and not condition else ""))
    if not condition:
        failures.append(label)


def substitute(text, variables):
    return re.sub(r"\$(ZED_[A-Za-z_]+)", lambda m: variables[m.group(1)], text)


def run(shell, task, variables, env):
    line = " ".join([substitute(task["command"], variables)] + [substitute(a, variables) for a in task["args"]])
    return subprocess.run([shell, "-c", line], capture_output=True, text=True, env=env, timeout=30)


work = pathlib.Path(tempfile.mkdtemp(prefix="renpy-tasks-"))
try:
    sdk = work / "renpy-8.9.9-sdk"
    sdk.mkdir()
    # The fake SDK prints its arguments, one per line.
    (sdk / "renpy.sh").write_text('#!/bin/sh\nfor a in "$@"; do echo "ARG:$a"; done\n')
    (sdk / "renpy.sh").chmod(0o755)
    shells = [s for s in ("sh", "bash", "zsh", "dash") if shutil.which(s)]
    for project in (work / "proj", work / "Ren'Py games" / "my game"):
        script = project / "game" / "chapter one.rpy"
        script.parent.mkdir(parents=True)
        script.write_text("label start:\n    return\n")
        variables = {"ZED_FILE": str(script), "ZED_ROW": "1", "ZED_CUSTOM_renpy_label": "start"}
        env = dict(os.environ, RENPY_SDK=str(sdk))
        expected = {
            "run": [str(project), "run"],
            "warp": [str(project), "run", "--warp", "game/chapter one.rpy:1"],
            "lint": [str(project), "lint"],
        }
        for shell in shells:
            for task in TASKS:
                kind = task["env"]["RENPY_ZED_TASK"]
                result = run(shell, task, variables, env)
                got = [l[4:] for l in result.stdout.splitlines() if l.startswith("ARG:")]
                check(
                    f"{shell}: {substitute(task['label'], variables)} in {project.name!r}",
                    result.returncode == 0 and got == expected[kind],
                    f"exit {result.returncode}, args {got}, stderr {result.stderr.strip()[-200:]}",
                )
    # Without RENPY_SDK and with no SDK anywhere, the task explains itself.
    env = {"PATH": os.environ["PATH"], "HOME": str(work / "empty-home")}
    result = run("sh", TASKS[0], {"ZED_FILE": str(work / "proj" / "game" / "chapter one.rpy"), "ZED_ROW": "1"}, env)
    check("a missing SDK is reported", result.returncode == 1 and "No Ren-Py SDK found" in result.stderr, result.stderr)
finally:
    shutil.rmtree(work, ignore_errors=True)

if failures:
    print(f"\n{len(failures)} FAILURE(S)")
    sys.exit(1)
print("\nALL CHECKS PASSED")
