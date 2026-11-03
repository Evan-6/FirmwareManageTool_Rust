#!/usr/bin/env python3
"""Check that each hand-written binding names every symbol the C ABI exports.

The header is the single list: `sdk/ffi/include/gdb.h` declares the symbols,
`sdk/ffi/src/lib.rs` is what actually leaves the DLL, and the C++, Python and Go
bindings only translate names. A capability added to the header and the FFI but
forgotten in one binding compiles everywhere and fails at run time in that one
language, which is the failure this gate exists for.

Three checks, none of them need a board:
  1. every `#[no_mangle] pub extern "C" fn gdb_*` in the FFI is declared in the
     header, and every declaration (other than a `static inline` helper) is
     exported;
  2. every exported symbol appears as a literal in all three bindings;
  3. the layouts that are passed by pointer (`gdb_metrics`, `gdb_clamps`,
     `gdb_move_report`) carry the same field names in each binding, and every
     capability that is not a symbol (screen constructors, monitor listing, clamp
     readout) has a name.
"""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
HEADER = ROOT / "sdk/ffi/include/gdb.h"
FFI = ROOT / "sdk/ffi/src/lib.rs"
BINDINGS = {
    "python": ROOT / "sdk/python/gdb.py",
    "go": ROOT / "sdk/go/gdb.go",
    "cpp": ROOT / "sdk/cpp/gdb.hpp",
}

failures = []


def fail(message):
    failures.append(message)
    print(f"FAIL {message}")


def read(path):
    return path.read_text(encoding="utf-8")


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"//.*", " ", text)


def header_declarations():
    """Every function the header declares, and the ones it defines itself."""
    text = strip_comments(read(HEADER))
    declared, inline = set(), set()
    for match in re.finditer(r"\b(gdb_[a-z0-9_]+)\s*\(", text):
        name = match.group(1)
        line_start = text.rfind("\n", 0, match.start()) + 1
        prefix = text[line_start : match.start()]
        declared.add(name)
        if "static inline" in prefix:
            inline.add(name)
    return declared, inline


def exported():
    """Every symbol the DLL actually exports, wherever in the FFI crate it lives."""
    names = set()
    for source in sorted((ROOT / "sdk/ffi/src").rglob("*.rs")):
        for match in re.finditer(
            r'#\[unsafe\(no_mangle\)\]\s*pub extern "C" fn (gdb_[a-z0-9_]+)',
            source.read_text(encoding="utf-8"),
        ):
            names.add(match.group(1))
    return names


def header_struct(name):
    """Field names of `typedef struct NAME { ... } NAME;`."""
    text = strip_comments(read(HEADER))
    block = re.search(r"typedef struct " + name + r"\s*\{(.*?)\}", text, flags=re.S)
    if not block:
        fail(f"header has no struct {name}")
        return []
    fields = []
    for line in block.group(1).splitlines():
        field = re.match(r"\s*[A-Za-z_][A-Za-z0-9_ ]*?\s+([a-z_][a-z0-9_]*)\s*;", line)
        if field:
            fields.append(field.group(1))
    return fields


def go_name(field):
    """The exported Go field a C field of the same name becomes."""
    return "".join(part[:1].upper() + part[1:] for part in field.split("_"))


def binding_block(language, type_name):
    """The text a binding declares `type_name` in, so field names can be looked up."""
    text = read(BINDINGS[language])
    if language == "python":
        block = re.search(
            r"class " + type_name + r"\(ctypes\.Structure\):(.*?)(?=\nclass |\n_D\.|\ndef )",
            text,
            flags=re.S,
        )
    elif language == "go":
        block = re.search(
            r"type " + type_name + r" struct \{(.*?)\n\}", text, flags=re.S
        )
    else:
        block = re.search(
            r"struct " + type_name + r" \{(.*?)\n\};", text, flags=re.S
        )
    return block.group(1) if block else ""


# Structs passed by pointer: the caller allocates them, so a field the binding
# does not declare is memory written past the end of its struct.
STRUCTS = {
    "gdb_metrics": {"python": "Metrics", "go": "Metrics", "cpp": "Metrics"},
    "gdb_clamps": {"python": "Clamps", "go": "Clamps", "cpp": "Clamps"},
    "gdb_move_report": {
        # The Python binding hands the caller a frozen dataclass and keeps the
        # ctypes mirror private, so the layout check looks at that struct.
        "python": "_MoveReport",
        "go": "MoveReport",
        "cpp": "MoveReport",
    },
    "gdb_flash_stats": {
        "python": "FlashStats",
        "go": "rawFlashStats",
        "cpp": "FlashStats",
    },
    "gdb_replay_outcome": {
        "python": "ReplayOutcome",
        "go": "rawReplayOutcome",
        "cpp": "ReplayOutcome",
    },
}

# Capabilities that are not one symbol each: a constructor or a method name has
# to exist in every binding even though the ABI names nothing.
CAPABILITIES = {
    "screen containing": {
        "python": "def containing(",
        "go": "func ScreenContaining(",
        "cpp": "static Screen containing(",
    },
    "monitor listing": {
        "python": "def monitors(",
        "go": "func Monitors(",
        "cpp": "monitors_json(",
    },
    "clamp counters": {
        "python": "def clamps(",
        "go": ") Clamps(",
        "cpp": "clamps() const",
    },
}


def minor_version():
    """The ABI minor revision the header declares."""
    found = re.search(r"#define GDB_ABI_VERSION_MINOR (\d+)u", read(HEADER))
    if not found:
        fail("gdb.h has no GDB_ABI_VERSION_MINOR")
        return None
    return found.group(1)


def binding_minor(language, text):
    pattern = {
        "python": r"^ABI_VERSION_MINOR\s*=\s*(\d+)",
        "go": r"^\s*ABIMinor\s*=\s*(\d+)",
        "cpp": r"#include",
    }[language]
    if language == "cpp":
        # The C++ header reads GDB_ABI_VERSION_MINOR straight out of gdb.h.
        return minor_version() if "GDB_ABI_VERSION_MINOR" in text else None
    found = re.search(pattern, text, flags=re.M)
    return found.group(1) if found else None


def main():
    declared, inline = header_declarations()
    symbols = exported()
    print(f"abi  {len(symbols)} exported symbols, {len(declared)} declarations")

    for name in sorted(symbols - declared):
        fail(f"{name} is exported but not declared in gdb.h")
    for name in sorted(declared - inline - symbols):
        fail(f"gdb.h declares {name} but the FFI does not export it")

    sources = {language: read(path) for language, path in BINDINGS.items()}
    for name in sorted(symbols):
        for language, text in sources.items():
            if name not in text:
                fail(f"{language} binding never names {name}")

    minor = minor_version()
    if minor is not None:
        for language, text in sources.items():
            reported = binding_minor(language, text)
            if reported != minor:
                fail(
                    f"{language} binding reports ABI minor {reported}, "
                    f"the header says {minor}"
                )

    for struct, names in STRUCTS.items():
        fields = header_struct(struct)
        for language, type_name in names.items():
            block = binding_block(language, type_name)
            if not block:
                fail(f"{language} binding has no {type_name} for {struct}")
                continue
            for field in fields:
                wanted = go_name(field) if language == "go" else field
                if wanted not in block:
                    fail(f"{language} {type_name} is missing {struct}.{field}")

    for capability, names in CAPABILITIES.items():
        for language, needle in names.items():
            if needle not in sources[language]:
                fail(f"{language} binding has no name for {capability} ({needle})")

    if failures:
        print(f"\n{len(failures)} problem(s): the header, the FFI and the bindings disagree")
        return 1
    print("ok   every ABI symbol and layout is named in C++, Python and Go")
    return 0


if __name__ == "__main__":
    sys.exit(main())
