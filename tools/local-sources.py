#!/usr/bin/env python3
"""Print Cargo patches for coordinated unpublished NeoCogi workspace changes.

Redirect stdout to a temporary TOML file and pass it to Cargo with --config.
This does not modify manifests, lockfiles, global configuration or git branches.
"""
import argparse
import json
from pathlib import Path
import tomllib


def packages(root: Path):
    """Yield published library names and paths from explicit workspace members."""
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    members = workspace.get("workspace", {}).get("members", ["."])
    for pattern in members:
        for directory in sorted(root.glob(pattern)):
            manifest = directory / "Cargo.toml"
            data = tomllib.loads(manifest.read_text())
            package = data.get("package", {})
            library = "lib" in data or (directory / "src/lib.rs").is_file()
            if library and package.get("publish") is not False and "name" in package:
                yield package["name"], directory.resolve()


def main():
    """Resolve explicit checkouts and emit portable Cargo --config input."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, type=Path, help="also patch Platform packages for downstream UI builds")
    args = parser.parse_args()
    patches = {}
    for root in [args.platform]:
        if root is not None:
            for name, directory in packages(root.resolve()):
                if name in patches and patches[name] != directory:
                    parser.error(f"conflicting source for {name}")
                patches[name] = directory
    print("[patch.crates-io]")
    for name, directory in sorted(patches.items()):
        print(f"{name} = {{ path = {json.dumps(str(directory))} }}")


if __name__ == "__main__":
    main()
