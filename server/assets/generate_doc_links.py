#!/usr/bin/env python3
"""Regenerate renpy-doc-links.json from the Ren'Py docs' Sphinx inventory.

Maps every symbol present in renpy-docs.json to its page#anchor on
https://www.renpy.org/doc/html/. Run from anywhere:

    python3 server/assets/generate_doc_links.py [path-to-objects.inv]

Without an argument, downloads https://www.renpy.org/doc/html/objects.inv.
"""
import json
import pathlib
import sys
import urllib.request
import zlib

HERE = pathlib.Path(__file__).parent
INVENTORY_URL = "https://www.renpy.org/doc/html/objects.inv"


def read_inventory(raw: bytes):
    """Yield (name, domain_role, uri) from a Sphinx v2 inventory."""
    lines = raw.split(b"\n", 4)
    if not lines[0].startswith(b"# Sphinx inventory version 2"):
        raise SystemExit("unsupported inventory format")
    print("inventory:", lines[1].decode().strip("# \n"), "|", lines[2].decode().strip("# \n"))
    for line in zlib.decompress(lines[4]).decode().splitlines():
        parts = line.split(None, 4)
        if len(parts) < 4:
            continue
        name, domain_role, _priority, uri = parts[0], parts[1], parts[2], parts[3]
        if uri.endswith("$"):
            uri = uri[:-1] + name
        yield name, domain_role, uri


def main():
    if len(sys.argv) > 1:
        raw = pathlib.Path(sys.argv[1]).read_bytes()
    else:
        raw = urllib.request.urlopen(INVENTORY_URL).read()

    docs = json.loads((HERE / "renpy-docs.json").read_text())
    wanted = set()
    for section in docs.values():
        wanted.update(section.keys())

    # Collect all candidate URIs per wanted name, preferring the py: domain.
    candidates = {}
    for name, domain_role, uri in read_inventory(raw):
        if name in wanted:
            candidates.setdefault(name, []).append((domain_role, uri))
    links = {}
    for name, options in candidates.items():
        options.sort(key=lambda o: (not o[0].startswith("py:"), o[0]))
        links[name] = options[0][1]

    out = HERE / "renpy-doc-links.json"
    out.write_text(json.dumps(dict(sorted(links.items())), indent=0) + "\n")
    print(f"{len(links)} of {len(wanted)} documented symbols got links -> {out.name}")


if __name__ == "__main__":
    main()
