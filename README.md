# Ren'Py for Zed

Ren'Py visual novel script support for the [Zed](https://zed.dev) editor: highlighting, language intelligence, engine lint, and running or debugging the game, for `.rpy` and `.rpym` files.

## Features

- **Highlighting** of Ren'Py statements, with Python highlighting injected into `$` lines, `python:` blocks, `define`/`default` values and `if`/`while`/menu conditions.
- **Editing basics**: comment toggling, bracket matching and auto-closing (including triple quotes), 4-space auto-indentation with `elif`/`else` dedent, and an outline of labels, menus, `define`/`default`, images and `init python` blocks.
- **Navigation** across the whole project: go to definition for labels, speakers, variables, images, screens, transforms and styles; find references (precise for `jump`/`call` targets, textual for speakers and variables); project-wide symbol search.
- **Completion** that knows its context: labels after `jump`/`call`, screens after `show`/`call screen`, images after `show`/`scene`/`hide` (transforms and positions after `at`), keywords and speakers at the start of a line, and 1100+ Ren'Py built-ins with inline docs elsewhere.
- **Hover docs**: your own symbols show their definition line and the `#` comment block above it; built-ins show their signature and API docs from Ren'Py 8.3, with a link to the official documentation.
- **Rename labels** (F2) across files in one edit; renaming onto an existing label or a non-label is refused.
- **Diagnostics** as you type: undefined `jump`/`call` targets are errors, duplicate labels are warnings. Dynamic `jump expression` and local `.labels` are never flagged. With an SDK available, `renpy lint` also runs at startup and on save, and its findings (missing images, undefined speakers, style and translation problems, and more) appear as warnings.
- **Run the game** from Zed's debugger, with its output in the debug console, a working stop button, and warp-to-cursor.
- **Debug the game** (Ren'Py 8.2+): breakpoints on any Ren'Py statement and inside Python, step over/into/out at statement and Python-line granularity, locals and the whole store in the Variables panel (editable), the call stack, and a console that evaluates expressions against the live game.

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

Add configurations to `.zed/debug.json` in your project and start one from the debug panel (`f4`):

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
- On engines older than 8.2 the game runs normally and the console notes that breakpoints are inactive.

## Known limitations

- `[variable]` interpolation and `{b}text tags{/b}` inside dialogue strings are highlighted as part of the string, not separately; the grammar treats strings as opaque tokens.
- `screen` / `transform` / `style` / `translate` blocks are not yet in the grammar (planned upstream for v0.5.0). They degrade gracefully: surrounding code still parses and highlights.
- Python injection is per-line inside python blocks; coloring is correct, but there are no cross-line semantics.

## Development

- **Language server:** a custom-built, MIT-licensed server in [server/](server/). It indexes definition sites with a line-based scan of every `.rpy`/`.rpym` file at startup and re-indexes files as you edit; the same binary serves the debug adapter in `dap` mode.
- **Tests:** `cargo test --manifest-path server/Cargo.toml`; after a release build, `python3 tests/e2e_lsp.py`, `python3 tests/e2e_dap.py` and `python3 tests/e2e_debug_real.py` (the last needs a Ren'Py SDK and opens a game window briefly).
- **Grammar:** the tree-sitter grammar lives in [grammar/](grammar/), vendored from [ZeynTheDev/tree-sitter-renpy](https://github.com/ZeynTheDev/tree-sitter-renpy) v0.4.0 (MIT) and maintained here. See [grammar/README.md](grammar/README.md) for the change and re-pin workflow.
- **Built-in docs:** `server/assets/renpy-docs.json` is vendored from the MIT-licensed [vscode-language-renpy](https://github.com/LuqueDaniel/vscode-language-renpy) project (license copy in `server/assets/renpy-docs-LICENSE`) and generated from Ren'Py 8.3's documentation. To refresh, re-download `src/renpy.json` from that project, bump `RENPY_DOCS_VERSION` in `server/src/main.rs`, and regenerate the renpy.org links with `server/assets/generate_doc_links.py`.
- **Releasing the server:** bump `version` in `server/Cargo.toml` and `SERVER_RELEASE_TAG` in `src/lib.rs` together (CI fails when they disagree), then push a `v<version>` tag. The release workflow builds every platform into a draft release and publishes it once all assets are uploaded; it refuses a tag that does not match the crate version. Bump `version` in `extension.toml` as well when the extension itself is republished.

## License

MIT, see [LICENSE](LICENSE). The grammar is MIT-licensed by its author.
