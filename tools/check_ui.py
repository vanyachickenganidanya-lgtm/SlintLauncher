#!/usr/bin/env python3
"""Compile the Slint UI and cross-check it against the Rust bindings.

This does not need a Rust toolchain: the `slint` PyPI package embeds the same
compiler used by `slint-build`, so UI syntax errors and Rust/Slint interface
mismatches are caught early.

    pip install slint
    python3 tools/check_ui.py
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

try:
    import slint
except ImportError:  # pragma: no cover
    print("error: pip install slint", file=sys.stderr)
    raise SystemExit(2)


def main() -> int:
    problems: list[str] = []

    try:
        component = slint.load_file(str(ROOT / "ui/app.slint"), style="fluent-dark")
    except slint.CompileError as exc:
        print("Slint compilation failed:\n", exc, file=sys.stderr)
        return 1

    window = component.AppWindow
    members = {name for name in dir(window) if not name.startswith("_")}
    rust = (ROOT / "src/main.rs").read_text()

    # 1. every ui.set_/get_/on_/invoke_ target must exist on the component
    for match in re.finditer(r"\bui\.(set|get|on|invoke)_([a-z0-9_]+)\(", rust):
        kind, name = match.group(1), match.group(2)
        if name not in members:
            problems.append(f"AppWindow has no member '{name}' (used as ui.{kind}_{name})")

    # 2. struct literals in Rust must use exactly the declared Slint fields
    types_src = (ROOT / "ui/types.slint").read_text()

    def declared_fields(struct: str) -> set[str]:
        body = re.search(rf"export struct {struct} \{{(.*?)\n\}}", types_src, re.S)
        if not body:
            problems.append(f"struct {struct} not found in ui/types.slint")
            return set()
        return {
            field.replace("-", "_")
            for field in re.findall(r"^\s*([a-z][a-z0-9\-]*)\s*:", body.group(1), re.M)
        }

    for struct in ("InstanceItem", "AccountItem", "VersionItem", "LogLine"):
        declared = declared_fields(struct)
        for literal in re.finditer(struct + r"\s*\{(.*?)\n\s*\}", rust, re.S):
            used = set(re.findall(r"^\s*([a-z][a-z0-9_]*)\s*:", literal.group(1), re.M))
            if unknown := used - declared:
                problems.append(f"{struct}: unknown field(s) {sorted(unknown)}")
            if absent := declared - used:
                problems.append(f"{struct}: field(s) never set {sorted(absent)}")

    if problems:
        print("Problems found:")
        for problem in sorted(set(problems)):
            print(" -", problem)
        return 1

    print(f"OK: ui/app.slint compiles, {len(members)} members match src/main.rs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
