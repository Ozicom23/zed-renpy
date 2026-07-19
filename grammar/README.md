# tree-sitter-renpy (vendored)

The tree-sitter grammar for Ren'Py used by this extension, maintained in-repo.

**Provenance:** vendored from [ZeynTheDev/tree-sitter-renpy](https://github.com/ZeynTheDev/tree-sitter-renpy) at commit `8a98470c0eba8d9c41d12e5a75118fa6aed4cfb7` (v0.4.0, MIT — see [LICENSE](LICENSE)), byte-identical at the time of vendoring. It is maintained here independently from now on.

## Layout

- `grammar.js` — the grammar definition (edit this)
- `src/scanner.c` — external scanner handling INDENT/DEDENT/NEWLINE (edit if block structure changes)
- `src/parser.c`, `src/grammar.json`, `src/node-types.json` — **generated** by the tree-sitter CLI; must be committed (Zed compiles `src/parser.c` directly and never runs `tree-sitter generate`)

## Making changes

1. Edit `grammar.js` (and/or `src/scanner.c`).
2. Regenerate: `npx tree-sitter-cli@latest generate` (run inside `grammar/`).
3. Re-validate every query in `../languages/renpy/*.scm` against the new node names — a stale node name makes the whole query fail to compile and Zed then renders **no highlighting at all**. Parse `../examples/test.rpy` and expect zero ERROR nodes.
4. Commit the grammar change, then update `commit` in `../extension.toml` to that commit's SHA (the extension pins this repo itself — two-step commit).
