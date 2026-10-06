#!/usr/bin/env python3
"""Package a natively built Windows MSVC executable on Windows."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    options = parser.parse_args()
    if sys.platform != "win32":
        parser.error("Windows EXE/ZIP packaging must run on Windows. Use scripts/build.ps1 or scripts/package.ps1 on Windows.")
    root = Path(__file__).resolve().parent.parent
    binary = options.binary or root / "target/x86_64-pc-windows-msvc/release/firmware-manage-tool.exe"
    if not binary.is_file():
        parser.error("Build the Windows MSVC executable first.")
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=root))
    project = next(p for p in metadata["packages"] if p["name"] == "firmware-manage-tool")
    name = f"FirmwareManageTool_Rust-{project['version']}-windows-x64"
    dist = root / "dist"
    stage = dist / name
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    shutil.copy2(binary, stage / "FirmwareManageTool.exe")
    for name in ("firmware", "shared", "docs", "README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"):
        path = root / name
        if path.is_dir():
            shutil.copytree(path, stage / name)
        else:
            shutil.copy2(path, stage / name)
    licenses = stage / "ThirdPartyLicenses"
    licenses.mkdir()
    notices = []
    for package in metadata["packages"]:
        notices.append({key: package.get(key) for key in ("name", "version", "license", "repository", "source")})
        if package["name"] == "firmware-manage-tool":
            continue
        source = Path(package["manifest_path"]).parent
        for item in source.iterdir():
            if not re.match(r"^(LICENSE|LICENCE|COPYING|NOTICE)([-._].*)?$", item.name, re.I):
                continue
            destination = licenses / f"{package['name']}-{package['version']}"
            destination.mkdir(exist_ok=True)
            if item.is_dir():
                shutil.copytree(item, destination / item.name)
            else:
                shutil.copy2(item, destination / item.name)
        if package["name"] == "hidapi":
            destination = licenses / "hidapi-c"
            destination.mkdir(exist_ok=True)
            for item in (source / "etc/hidapi").glob("LICENSE*"):
                shutil.copy2(item, destination / item.name)
    (licenses / "dependencies.json").write_text(json.dumps(notices, ensure_ascii=False, indent=2), encoding="utf-8")
    archive = dist / (stage.name + ".zip")
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9, strict_timestamps=False) as out:
        for file in sorted(stage.rglob("*")):
            if file.is_file():
                out.write(file, file.relative_to(dist))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (dist / (archive.name + ".sha256")).write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    print(f"Portable package: {archive}\nSHA256: {digest}")


if __name__ == "__main__":
    main()
