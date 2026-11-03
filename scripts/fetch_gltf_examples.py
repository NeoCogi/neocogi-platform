#!/usr/bin/env python3
"""Fetch pinned glTF models and the reference HDR; verify before replacing files.

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
ENVIRONMENT_REVISION = "3a030b54c2e4f0f4d469eec2f98f47398c9a2a30"
ENVIRONMENT_SHA256 = "7d65926ed2fa41c121738af2addf1fd5f1ae42d12ff77255e2679777f06c984b"


def verified_download(path: Path, url: str, digest: str) -> None:
    """Keep verified files and install only checksum-verified bytes using an atomic rename."""
    if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == digest:
        print(f"Verified {path}")
        return
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f"SHA-256 mismatch for {path.name}; existing file preserved")
    temporary = path.with_suffix(path.suffix + ".part")
    temporary.write_bytes(data)
    temporary.replace(path)
    print(f"Downloaded {path}")


def fetch(destination: Path) -> None:
    """Fetch models and the external Cannon Exterior lighting source without build-time I/O."""
    destination.mkdir(parents=True, exist_ok=True)
    for model, digest in MODELS.items():
        url = f"https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/{REVISION}/Models/{model}/glTF-Binary/{model}.glb"
        verified_download(destination / f"{model}.glb", url, digest)
    url = f"https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Environments/{ENVIRONMENT_REVISION}/Cannon_Exterior.hdr"
    verified_download(destination / "Cannon_Exterior.hdr", url, ENVIRONMENT_SHA256)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "crates/platform/plugin/examples/gltf/assets")
    fetch(parser.parse_args().output)
