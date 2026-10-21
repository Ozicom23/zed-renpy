# Ren'Py for Zed

Ren'Py visual novel script support for the [Zed](https://zed.dev) editor: highlighting, language intelligence, engine lint, and running or debugging the game, for `.rpy` and `.rpym` files.

## Features

- **Highlighting** of the whole language: script statements, screens, transforms and ATL, styles, layered images and translations. Python is highlighted inside `$` lines, `python:` blocks, `define`/`default` values, conditions and screen/ATL arguments. In dialogue, `[interpolation]`, `{text tags}` and escapes get their own colors.
- **Editing basics**: comment toggling, bracket matching and auto-closing (including triple quotes), 4-space auto-indentation with `elif`/`else` dedent, snippets (`menu`, `character`, `screen`, `textbutton`, `layeredimage` and more), and an outline of labels, menus, screens, transforms, styles, images, `define`/`default` and translations.
- **Navigation** across the whole project: go to definition for labels, speakers, variables, images, screens, transforms and styles; find references (precise for `jump`/`call` targets, textual for speakers and variables); project-wide symbol search.
- **Images from files**: like Ren'Py, every image file under `game/images/` defines an image named after it (`bg room.png` is `bg room`). They are completed after `show`/`scene`, go to definition opens the file, and hover shows a preview.
- **Completion** that knows its context: labels after `jump`/`call`, screens after `show`/`call screen`, images after `show`/`scene`/`hide` (transforms and positions after `at`), keywords and speakers at the start of a line, and 1100+ Ren'Py built-ins with inline docs elsewhere.
- **Parameter hints** while typing a call: built-ins like `Character(`, `renpy.pause(` and actions, and your own labels, screens and transforms.
- **Hover docs**: your own symbols show their definition line and the `#` comment block above it; built-ins show their signature and API docs from Ren'Py 8.3, with a link to the official documentation.
- **Script statistics**: hovering a character shows how many lines and words they speak, hovering a label shows its dialogue length and reading time, and the "Write Ren'Py script statistics" code action writes a full report (per character, file and label) to `script_stats.md`.
- **Color swatches** beside hex colors such as `"#c8c8ff"`, with a color picker.
- **Rename labels** (F2) across files in one edit; renaming onto an existing label or a non-label is refused.
- **Diagnostics** as you type: undefined `jump`/`call` targets are errors, duplicate labels are warnings, and `show`/`scene`/`hide` of an image tag nothing defines is a warning. Dynamic `jump expression` and local `.labels` are never flagged; the image check turns itself off in projects that define images with computed names (`renpy.image(name, ...)`). With an SDK available, `renpy lint` also runs at startup and on save, and its findings (missing images, undefined speakers, style and translation problems, and more) appear as warnings.
- **Quick fixes**: change a misspelled label or image to the closest match, create a missing label, or rename a duplicate one.
- **Run buttons** beside every label: run the game from that label, or debug it. Tasks for running the game, running from the cursor's line and `renpy lint` are in the task picker too.
- **Run the game** from Zed's debugger, with its output in the debug console, a working stop button, and warp-to-cursor.
- **Debug the game** (Ren'Py 8.2+): breakpoints on any Ren'Py statement and inside Python, with conditions, hit counts and log messages; pausing on script errors before Ren'Py's traceback screen (optionally on every raised exception); step over/into/out at statement and Python-line granularity; locals and the whole store in the Variables panel (editable); the call stack; and a console that evaluates expressions against the live game.

## Installation

**From the Zed registry:** Not supported, pending review in zed extensions [PR#6873](https://github.com/zed-industries/extensions/pull/6873)

**From source:** clone this repository, then in Zed run `zed: extensions`, choose **Install Dev Extension** and select the folder. The first install compiles the grammar and the Rust glue to WASM, which needs a Rust toolchain via rustup. The server is still downloaded automatically; to use a local build instead, run

```sh
cargo build --release --manifest-path server/Cargo.toml
```

and either put `server/target/release/renpy-language-server` on your PATH or point Zed at it (see below). Open `examples/test.rpy` to see every supported construct highlighted.

## Configuration

Everything here is optional. In Zed settings:

```json
"lsp": {
  "renpy-language-server": {
    "binary": { "path": "/absolute/path/to/renpy-language-server" },
    "initialization_options": {
      "sdk": "/path/to/renpy-8.5.3-sdk",
      "lint": true
    }
  }
}
```

The Ren'Py SDK is found, in order, from an explicit `sdk` setting, the `RENPY_SDK` environment variable, or a `renpy-*-sdk` directory in your home, `Documents`, `Downloads` or `Desktop` folder (newest version wins). Set `"lint": false` to turn engine lint off.

## Running and debugging

The quickest way is the run button beside a label, which offers both running and debugging from there. The tasks need a shell (`sh`), so on Windows use the debug variants or a debug configuration.

For more control, add configurations to `.zed/debug.json` in your project and start one from the debug panel (`f4`):

```json
[
  { "adapter": "renpy", "label": "Ren'Py: run game", "request": "launch" },
  { "adapter": "renpy", "label": "Ren'Py: run from cursor", "request": "launch", "warp": "$ZED_FILE:$ZED_ROW" }
]
```

`warp` starts the game directly at the statement under the cursor (needs `config.developer`, which is on by default during development). Optional fields: `sdk` (same discovery as above), `project` when your `game/` directory is not the worktree root, `command` (default `run`; also `lint`, `compile`), `args`, and `env` (e.g. `{"RENPY_SKIP_SPLASHSCREEN": "1"}`).

Debugging notes:

- A small agent, `game/zed_debug.rpe.py`, is written into the project when a session starts and deleted when it ends. Ren'Py never ships `.rpe*` files in built distributions, and without the session's environment variables the file does nothing.
- The game pauses *before* a breakpointed statement runs. Breakpoints on lines that are not executable statements never hit.
- Pausing with ⏸ lands at the next executed statement; while the game waits for a click nothing executes, so advance it once.
- Values typed in the Variables panel and the console are Python expressions, so `points + 50` works, and statements such as `inventory.append("sword")` run too. Function-frame locals are read-only on Python 3.12 (Ren'Py 8.5 and earlier); script-level Python and the store are always writable.
- Right-click a breakpoint to give it a condition (`points > 10`), a hit count (`3`, `>=3`, `%3`) or a log message (`gold is {gold}`), which prints to the debug console instead of pausing.
- **Script errors** (on by default in the breakpoints list) pauses where a script error is raised, with its frames inspectable, before Ren'Py shows its traceback screen. **All raised exceptions** also pauses on exceptions the game catches itself; Ren'Py's own scripts and its control flow (jump, call, restart) are never reported.
- On engines older than 8.2 the game runs normally and the console notes that breakpoints are inactive.

## Known limitations

- Screen-language, ATL and style lines share one generic grammar shape (a name, arguments, an optional block). Keywords and properties are told apart by name, so a property Ren'Py adds later is highlighted as a plain name until the list is updated.
- Python injection is per-line inside python blocks; coloring is correct, but there are no cross-line semantics.
- The language server reads scripts line by line; it does not evaluate Python, so names defined only in Python (`renpy.image(name, ...)`, `setattr(store, ...)`) are invisible to completion and diagnostics.

## Development

- **Language server:** a custom-built, MIT-licensed server in [server/](server/). It indexes definition sites with a line-based scan of every `.rpy`/`.rpym` file at startup and re-indexes files as you edit; the same binary serves the debug adapter in `dap` mode.
- **Tests:** `cargo test --manifest-path server/Cargo.toml`; after a release build, `python3 tests/e2e_lsp.py`, `python3 tests/e2e_dap.py`, `python3 tests/test_tasks.py` and `python3 tests/e2e_debug_real.py` (the last needs a Ren'Py SDK and opens a game window briefly).
- **Grammar:** the tree-sitter grammar lives in [grammar/](grammar/), vendored from [ZeynTheDev/tree-sitter-renpy](https://github.com/ZeynTheDev/tree-sitter-renpy) v0.4.0 (MIT) and extended here. Every `.rpy` file of the Ren'Py SDK's projects and `renpy/common` parses without errors. See [grammar/README.md](grammar/README.md) for the change and re-pin workflow.
- **Built-in docs:** `server/assets/renpy-docs.json` is vendored from the MIT-licensed [vscode-language-renpy](https://github.com/LuqueDaniel/vscode-language-renpy) project (license copy in `server/assets/renpy-docs-LICENSE`) and generated from Ren'Py 8.3's documentation. To refresh, re-download `src/renpy.json` from that project, bump `RENPY_DOCS_VERSION` in `server/src/main.rs`, and regenerate the renpy.org links with `server/assets/generate_doc_links.py`.
- **Releasing the server:** bump `version` in `server/Cargo.toml` and `SERVER_RELEASE_TAG` in `src/lib.rs` together (CI fails when they disagree), then push a `v<version>` tag. The release workflow builds every platform into a draft release and publishes it once all assets are uploaded; it refuses a tag that does not match the crate version. Bump `version` in `extension.toml` as well when the extension itself is republished.

## License

MIT, see [LICENSE](LICENSE). The grammar is MIT-licensed by its author.
