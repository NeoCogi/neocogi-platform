#!/usr/bin/env python3
"""Build both thin link roots in one Cargo graph and stage executable-relative libraries."""

import argparse
import filecmp
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def shared_libraries(messages):
    """Select exact Cargo-reported final outputs, never inferred hashes or timestamps."""
    selected = {}
    for message in messages:
        if message.get("reason") != "compiler-artifact":
            continue
        target = message["target"]
        if target["name"] not in {"nc_platform", "nc_shader_compiler"} or "cdylib" not in target["crate_types"]:
            continue
        files = [Path(name) for name in message["filenames"] if Path(name).suffix in {".so", ".dll", ".dylib"}]
        if len(files) != 1:
            raise ValueError(f"expected one shared library for {target['name']}: {files}")
        selected[target["name"]] = files[0]
    if set(selected) != {"nc_platform", "nc_shader_compiler"}:
        raise ValueError(f"Cargo did not supply both final shared libraries: {selected}")
    return selected


def install(source, destination):
    """Keep equal files stable and atomically replace changed final libraries."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    if (destination.is_file() and stat.S_IMODE(source.stat().st_mode) == stat.S_IMODE(destination.stat().st_mode)
            and filecmp.cmp(source, destination, shallow=False)):
        return
    with tempfile.TemporaryDirectory(prefix=".neocogi-plugin-", dir=destination.parent) as temporary:
        candidate = Path(temporary) / destination.name
        shutil.copy2(source, candidate)
        os.replace(candidate, destination)


def build(*, backend, profile="dev", target=None, offline=False, output=None):
    """Build outside application build scripts; the caller owns all Cargo orchestration."""
    command = ["cargo", "build", "--locked", "-p", "neocogi-platform-dylib", "--lib", "--example", "nc_shader_compiler",
               "--no-default-features", "--features", backend, "--profile", profile, "--message-format=json-render-diagnostics"]
    if target:
        command += ["--target", target]
    if offline:
        command.append("--offline")
    result = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True)
    libraries = shared_libraries(json.loads(line) for line in result.stdout.splitlines() if line.startswith("{"))
    root = Path(output or os.environ.get("NEOCOGI_NATIVE_OUTPUT_DIR") or libraries["nc_platform"].parent)
    if not root.is_absolute():
        raise ValueError("native output directory must be absolute")
    for name, source in libraries.items():
        filename = source.name.replace(name, f"{name}_{backend}")
        install(source, root / "lib" / filename)
        print(root / "lib" / filename)
    return root


def main():
    """Expose backend/profile/target choices without starting nested application builds."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=["glow", "vulkan", "wgpu"], default="glow")
    parser.add_argument("--profile", default="dev")
    parser.add_argument("--target")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--output", type=Path)
    build(**vars(parser.parse_args()))


if __name__ == "__main__":
    main()
