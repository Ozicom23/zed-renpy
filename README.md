# Ren'Py for Zed

Ren'Py visual novel script support for the [Zed](https://zed.dev) editor: syntax highlighting and core language features for `.rpy` and `.rpym` files.

## Features

- **Syntax highlighting** for Ren'Py script: labels, dialogue (speakers, say attributes), flow control (`jump`, `call`, `menu`), displayables (`scene`, `show`, `hide`, `image`), audio (`play`, `stop`, `queue` with fade modifiers), transitions, `define`/`default`, and more.
- **Python highlighting via language injection** inside `$ ...` one-liners, `python:` / `init python:` blocks, `define`/`default` values, and `if`/`elif`/`while`/menu-choice conditions.
- **Comment toggling** (`cmd-/`) using `# ` line comments.
- **Bracket auto-closing** for `()`, `[]`, `{}`, quotes, and triple quotes.
- **Auto-indentation** for colon-terminated blocks (4 spaces, the Ren'Py convention), including `elif`/`else` dedenting.
- **Outline view** (`cmd-shift-o`) listing labels, menus, `define`/`default`, images, and `init python` blocks.
- **Bracket matching.**
- **Go to definition** (cmd-click / F12) for labels (`jump`/`call` targets), speakers, `define`/`default` variables, images, screens, transforms, and styles — across the whole project, via the bundled language server.
- **Project-wide symbol search** (`cmd-t`) over all Ren'Py definitions.
- **Completion**, context-aware: label names after `jump`/`call`, screen names after `show screen`/`call screen`, image names after `show`/`scene`/`hide` (transforms and positions after `at`), statement keywords and speakers at the start of a line, and 1100+ Ren'Py built-ins with inline docs everywhere else.
- **Diagnostics** as you type: errors on `jump`/`call` targets that aren't defined anywhere in the project, warnings on duplicate label definitions. Deliberately conservative — dynamic `jump expression` and local `.labels` are never flagged.
- **Find references**: every `jump`/`call` to a label project-wide (precise), and word-boundary usages for speakers and variables (textual).
- **Rename labels** (F2): the definition and every `jump`/`call` update in one atomic edit across files; renaming onto an existing label or a non-label is refused.
- **Hover documentation**: your own symbols show their definition line plus any `#` comment block above it; Ren'Py built-ins (`Transform`, `Character`, `renpy.*`, `config.*`, …) show their signature and API docs from a bundled dataset of 1100+ entries targeting **Ren'Py 8.3** (each popup states this), plus a direct link to that symbol's page in the official documentation.
- **Run the game from the editor** via Zed's debugger: launch configurations start your project through the Ren'Py SDK, stream its output into the debug console, and stop it with the stop button — including **warp-to-cursor**, which boots the game directly at the line you're editing.
- **Engine lint as diagnostics**: on every save, `renpy lint` runs against your project (when an SDK is available) and its findings — missing images, undefined characters, and every other engine-level check — appear as warnings in the editor, merged with the built-in diagnostics.

## Installation

**From the Zed registry** (once published): Extensions (`cmd-shift-x`) → search "Ren'Py" → Install. The language-server binary is downloaded automatically from this repo's GitHub releases on first use; override it any time with `lsp.renpy-language-server.binary.path` in settings, or by putting `renpy-language-server` on PATH.

**From source (dev):**

1. Clone this repository.
2. In Zed: command palette → `zed: extensions` → **Install Dev Extension** → select the cloned folder. The first install compiles the grammar and the Rust glue to WASM (requires a Rust toolchain via rustup).
3. Build the server — `cargo build --release --manifest-path server/Cargo.toml` — then point `lsp.renpy-language-server.binary.path` at the built binary (or put it on PATH).

Open `examples/test.rpy` to see every supported construct highlighted.

## Language server (go to definition)

The repo bundles a minimal MIT-licensed language server in [server/](server/). It scans every `.rpy`/`.rpym` file in the workspace at startup, indexes definition sites (`label`, named `menu`, `define`, `default`, `image`, `screen`, `transform`, `style`), re-indexes files as you edit, and serves `textDocument/definition` and `workspace/symbol`.

Build it once (requires a Rust toolchain):

```sh
cargo build --release --manifest-path server/Cargo.toml
```

Hover docs for Ren'Py built-ins come from `server/assets/renpy-docs.json`, generated from **Ren'Py 8.3**'s official documentation and vendored from the MIT-licensed [vscode-language-renpy](https://github.com/LuqueDaniel/vscode-language-renpy) project (license copy: `server/assets/renpy-docs-LICENSE`). The version is a snapshot — to refresh, re-download `src/renpy.json` from that project and bump `RENPY_DOCS_VERSION` in `server/src/main.rs`. The per-symbol links to renpy.org come from `server/assets/renpy-doc-links.json`, generated from the official docs' Sphinx inventory by `server/assets/generate_doc_links.py` (re-run it to refresh).

Then either put `server/target/release/renpy-language-server` on your PATH, or point Zed at it in settings:

```json
"lsp": {
  "renpy-language-server": {
    "binary": { "path": "/absolute/path/to/zed-renpy/server/target/release/renpy-language-server" }
  }
}
```

## Running and debugging the game

The extension registers a `renpy` debug adapter (served by the same bundled binary, in `dap` mode). Add configurations to `.zed/debug.json` in your project:

```json
[
  {
    "adapter": "renpy",
    "label": "Ren'Py: run game",
    "request": "launch"
  },
  {
    "adapter": "renpy",
    "label": "Ren'Py: run from cursor",
    "request": "launch",
    "warp": "$ZED_FILE:$ZED_ROW"
  }
]
```

Start one from the debug panel (`f4` / `debugger: start`). The `warp` configuration launches the game **directly at the statement under your cursor** (Ren'Py's warp feature; needs `config.developer`, which is on by default during development).

How the SDK is found, in order: the `"sdk"` field in the configuration → the `RENPY_SDK` environment variable → a `renpy-*-sdk` directory in your home, `Documents`, `Downloads`, or `Desktop` folder (newest version wins). The project defaults to the worktree root; set `"project"` if your `game/` directory lives elsewhere. Other optional fields: `"command"` (default `run` — try `lint` or `compile`), `"args"`, and `"env"` (e.g. `{"RENPY_SKIP_SPLASHSCREEN": "1"}`).

Breakpoints are accepted but reported as unverified for now — real breakpoint support (inside `python:` blocks first, then on Ren'Py statements) is in development.

## Engine lint on save

With an SDK available (same discovery as above, or set explicitly), the language server runs `renpy lint` at startup and on every save, publishing each finding as a warning on its line. The engine checks far more than the built-in indexer can: missing image files, undefined speakers, style problems, translation issues, and more. Configure it in Zed settings:

```json
"lsp": {
  "renpy-language-server": {
    "initialization_options": {
      "sdk": "/path/to/renpy-8.5.3-sdk",  // optional if discoverable
      "lint": true                          // set false to disable
    }
  }
}
```

## Grammar

The tree-sitter grammar lives in this repo under [grammar/](grammar/) and is maintained here. It was originally vendored, byte-identical, from [ZeynTheDev/tree-sitter-renpy](https://github.com/ZeynTheDev/tree-sitter-renpy) v0.4.0 (MIT, license preserved). See [grammar/README.md](grammar/README.md) for the change/regeneration workflow.

## Known limitations

- `[variable]` interpolation and `{b}text tags{/b}` inside dialogue strings are highlighted as part of the string, not separately — the grammar currently treats strings as opaque tokens.
- `screen` / `transform` / `style` / `translate` blocks are not yet in the grammar (planned upstream for v0.5.0). They degrade gracefully: surrounding code still parses and highlights.
- Python injection is per-line inside python blocks; coloring is correct, but there are no cross-line semantics.

## License

MIT — see [LICENSE](LICENSE). The pinned grammar is MIT-licensed by its author.
