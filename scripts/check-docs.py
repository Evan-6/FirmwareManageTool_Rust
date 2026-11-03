#!/usr/bin/env python3
"""Check that the SDK documents agree with the library they describe.

`gdb.h` plus the exported symbols are the truth; the documents are the claim. A
feature that lands in the ABI but not in the table is invisible to a reader who
picks a language from it, and a row that promises a symbol the library does not
export is worse: the reader finds out at run time, in one language, with a
`GetProcAddress` failure. So both directions are checked, and neither needs a
board:

  1. every exported `gdb_*` symbol is named in `sdk/README.md`'s "What is
     covered" table, and every symbol the table names is exported;
  2. every exported symbol is documented in `sdk/API.md`, so the reference can
     answer "what does this call do";
  3. the capability JSON in `gdb.h` and `API.md` lists the same keys with the
     same values as the literals in `gdb_capabilities` (a key whose value is a
     platform expression must be documented as `true`, which is what Windows
     reports).
"""
from pathlib import Path
import json
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
HEADER = ROOT / "sdk/ffi/include/gdb.h"
FFI = ROOT / "sdk/ffi/src/lib.rs"
README = ROOT / "sdk/README.md"
API = ROOT / "sdk/API.md"

failures = []


def fail(message):
    failures.append(message)
    print(f"FAIL {message}")


def read(path):
    return path.read_text(encoding="utf-8")


def exported():
    """Every symbol the DLL actually exports."""
    names = set()
    for source in sorted((ROOT / "sdk/ffi/src").rglob("*.rs")):
        for match in re.finditer(
            r'#\[unsafe\(no_mangle\)\]\s*pub extern "C" fn (gdb_[a-z0-9_]+)',
            source.read_text(encoding="utf-8"),
        ):
            names.add(match.group(1))
    return names


def covered_table():
    """The C ABI column of the README's "What is covered" table.

    The table starts at its header row and ends at the first blank line after it,
    so a row that is deleted in one direction and a symbol that is added in the
    other are both visible here.
    """
    text = read(README)
    start = text.find("## What is covered")
    if start < 0:
        fail("README has no 'What is covered' section")
        return {}
    rows = {}
    seen_header = False
    for line in text[start:].splitlines():
        if not line.startswith("|"):
            if seen_header:
                break
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if not seen_header:
            if cells[:2] == ["Capability", "Rust"]:
                seen_header = True
                if len(cells) != 4:
                    fail(
                        "the covered table must have four columns "
                        "(Capability, Rust, C ABI symbols, bindings); it has "
                        f"{len(cells)}"
                    )
            continue
        if set(cells[0]) <= set("-: "):
            continue
        if len(cells) != 4:
            fail(f"covered table row has {len(cells)} cells: {line}")
            continue
        names = re.findall(r"`(gdb_[a-z0-9_]+)`", cells[2])
        if not names and cells[2] not in ("—", "-", ""):
            fail(
                "a C ABI column is neither '—' nor a list of symbols: "
                f"{cells[2]!r} in row {cells[0]!r}"
            )
        for name in names:
            if name in rows:
                fail(f"{name} is claimed twice: {rows[name]!r} and {cells[0]!r}")
            rows[name] = cells[0]
    if not seen_header:
        fail("the covered table has no '| Capability | Rust | … |' header")
    return rows


def capability_block(text, where):
    """The JSON object that starts with `{"click_timed"`, as a dict."""
    start = text.find('{"click_timed"')
    if start < 0:
        fail(f"{where} has no capability JSON example")
        return None
    # The examples wrap across lines; take everything up to the closing brace at
    # the start of a line.
    block = []
    for line in text[start:].splitlines():
        block.append(line.strip())
        if line.strip().endswith("}"):
            break
    else:  # pragma: no cover - a malformed document
        fail(f"{where}: the capability JSON never closes")
        return None
    try:
        return json.loads(" ".join(block))
    except json.JSONDecodeError as error:
        fail(f"{where}: the capability JSON does not parse: {error}")
        return None


def capability_literals():
    """The keys and literal values inside `gdb_capabilities` in the FFI.

    A value written as an expression (`windows`) is platform bound and is
    returned as None, so the documents are compared with what Windows reports.
    """
    text = read(FFI)
    start = text.find("fn gdb_capabilities")
    if start < 0:
        fail("the FFI has no gdb_capabilities")
        return {}
    block = text[start : text.find("\n}\n", start)]
    literals = {}
    for match in re.finditer(r'"([a-z_]+)"\s*:\s*(true|false|[A-Za-z_][A-Za-z0-9_]*)', block):
        key, value = match.group(1), match.group(2)
        literals[key] = None if value not in ("true", "false") else value == "true"
    if not literals:
        fail("could not read the capability literals out of gdb_capabilities")
    return literals


def check_capabilities():
    literals = capability_literals()
    if not literals:
        return
    for path, where in ((HEADER, "gdb.h"), (API, "API.md")):
        documented = capability_block(read(path), where)
        if documented is None:
            continue
        if set(documented) != set(literals):
            missing = sorted(set(literals) - set(documented))
            extra = sorted(set(documented) - set(literals))
            fail(f"{where} capability keys differ: missing {missing}, extra {extra}")
        for key, value in literals.items():
            if key not in documented:
                continue
            claimed = documented[key]
            if value is None:
                if claimed is not True:
                    fail(
                        f"{where} says {key}={claimed}, but the build reports it per "
                        "platform: the example must be the Windows value, true"
                    )
            elif claimed is not value:
                fail(f"{where} says {key}={claimed}, the build reports {value}")


def main():
    symbols = exported()
    if not symbols:
        fail("the FFI exports nothing: is sdk/ffi/src/lib.rs missing?")
    print(f"abi  {len(symbols)} exported symbols")

    rows = covered_table()
    print(f"doc  {len(rows)} symbols in the covered table")

    for name in sorted(symbols - set(rows)):
        fail(f"{name} is exported but the covered table does not name it")
    for name in sorted(set(rows) - symbols):
        fail(f"the covered table claims {name}, which the FFI does not export")

    api = read(API)
    for name in sorted(symbols):
        # A declaration, not just a mention: the reference has to show the call
        # with its arguments, which is also what makes the red test meaningful.
        if re.search(re.escape(name) + r"\s*\(", api) is None:
            fail(f"{name} is exported but API.md never declares it")

    check_capabilities()

    if failures:
        print(f"\n{len(failures)} problem(s): the documents and the library disagree")
        return 1
    print("ok   the covered table, API.md and the capability list match the library")
    return 0


if __name__ == "__main__":
    sys.exit(main())
