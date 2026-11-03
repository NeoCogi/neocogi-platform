#!/usr/bin/env python3
"""Fetch the two example GLBs at a fixed Khronos revision; verify before replacing files.

Models remain an explicit, local download rather than a build-time network dependency.
The adjacent checked-in license notices describe each upstream model's terms.
"""

import argparse
import hashlib
from pathlib import Path
import urllib.request


REVISION = "8f492390ddc7a2ae221a1369ef560589cf8c70fb"
MODELS = {
    "DamagedHelmet": "a1e3b04de97b11de564ce6e53b95f02954a297f0008183ac63a4f5974f6b32d8",
    "BrainStem": "ddda3a53b5c9771f992463034a430340ba2bac571e5869c723f7cba4976e9174",
}


def fetch(destination: Path) -> None:
    """Keep verified existing files, otherwise atomically install verified response bytes."""
    destination.mkdir(parents=True, exist_ok=True)
    for model, digest in MODELS.items():
        path = destination / f"{model}.glb"
        if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == digest:
            print(f"Verified {path}")
            continue
        url = f"https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/{REVISION}/Models/{model}/glTF-Binary/{model}.glb"
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"SHA-256 mismatch for {model}; existing file preserved")
        temporary = path.with_suffix(".glb.part")
        temporary.write_bytes(data)
        temporary.replace(path)
        print(f"Downloaded {path}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "crates/render/plugin/examples/gltf/assets")
    fetch(parser.parse_args().output)
