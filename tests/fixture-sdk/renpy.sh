#!/bin/sh
# Fake Ren'Py SDK launcher for e2e tests. Mirrors the real CLI shape:
#   renpy.sh <project> <command> [options...]
# Knobs: FAKE_RENPY_SLEEP (seconds to linger), FAKE_RENPY_EXIT (exit code).

if [ "$2" = "lint" ]; then
    # A BOM-prefixed report in the real format, ending in Statistics:.
    printf '\357\273\277'
    echo "Ren'Py 0.0.0 fake lint report, generated for tests"
    echo ""
    echo "game/script.rpy:3 fake lint problem from the fake SDK."
    echo "This second line continues the same problem."
    echo ""
    echo "Statistics:"
    echo ""
    echo "The game contains 0 dialogue blocks."
    exit 0
fi

echo "FAKE-RENPY ARGS: $*"
echo "fake renpy stdout line"
echo "fake renpy stderr line" >&2
if [ -n "$FAKE_RENPY_SLEEP" ]; then
    sleep "$FAKE_RENPY_SLEEP"
fi
exit "${FAKE_RENPY_EXIT:-0}"
