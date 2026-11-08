#!/usr/bin/env python3
"""Qualify real release archives and a consumer outside the repository; never upload."""

import argparse
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = {"neocogi-typed-buffer-tests", "neocogi-platform-examples", "neocogi-example-plugins", "neocogi-browser-tests"}
CONTRACTS_OWNER = "neocogi-platform-contracts"
RELEASE_ORDER = (CONTRACTS_OWNER, "neocogi-platform", "neocogi-platform-dylib")


def run(arguments, *, cwd=ROOT):
    """Run one foreground command; callers sequence all use of the shared target."""
    print("+", " ".join(str(arg) for arg in arguments), flush=True)
    subprocess.run(arguments, cwd=cwd, check=True)


def published_packages(metadata):
    """Select the declared crates.io release set and reject accidental private publication."""
    members = set(metadata["workspace_members"])
    selected = {}
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        if package["name"] in PRIVATE:
            if package["publish"] != []:
                raise ValueError(f"private package enabled for publication: {package['name']}")
        elif package["publish"] != ["crates-io"]:
            raise ValueError(f"release package must explicitly select crates.io: {package['name']}")
        else:
            selected[package["name"]] = package
    if CONTRACTS_OWNER not in selected:
        raise ValueError("shared source owner is missing from release set")
    if len({p["version"] for p in selected.values()}) != 1:
        raise ValueError("release members have inconsistent versions")
    if set(selected) != set(RELEASE_ORDER):
        raise ValueError("release set must contain exactly contracts, SDK and cdylib shim")
    return selected


def inspect_archive(archive, package):
    """Validate normalized manifests and safe members before extracting trusted Cargo output."""
    prefix = f"{package['name']}-{package['version']}"
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        for member in members:
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != prefix:
                raise ValueError(f"invalid archive path: {member.name}")
            if not (member.isfile() or member.isdir()):
                raise ValueError(f"archive contains nonregular member: {member.name}")
            if "OPEN_ITEMS" in member.name or "ADVERSARIAL" in member.name:
                raise ValueError("local review notes leaked into archive")
        names = {member.name for member in members}
        for required in ["Cargo.toml", "README.md", "LICENSE"]:
            if f"{prefix}/{required}" not in names:
                raise ValueError(f"missing {required} in {prefix}")
        manifest = tomllib.loads(bundle.extractfile(f"{prefix}/Cargo.toml").read().decode())
        if manifest["package"]["name"] != package["name"] or manifest["package"]["version"] != package["version"]:
            raise ValueError("archive package identity differs from selected metadata")
        tables = [manifest] + list(manifest.get("target", {}).values())
        for table in tables:
            for group in ["dependencies", "build-dependencies", "dev-dependencies"]:
                for name, dependency in table.get(group, {}).items():
                    if "path" in dependency or "git" in dependency or "workspace" in dependency:
                        raise ValueError(f"nonregistry dependency in {prefix}: {name}")
        schemas = [name for name in names if name.endswith(".ncidl")]
        if (package["name"] == CONTRACTS_OWNER and len(schemas) != 9) or (package["name"] != CONTRACTS_OWNER and schemas):
            raise ValueError(f"incorrect shared source ownership in {prefix}")
    return prefix


def qualify(*, allow_dirty=False, offline=False, stable=None):
    """Build archives, extract them, then use only extracted internal packages as dependencies."""
    flags = ["--offline"] if offline else []
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    cargo = ["cargo", f"+{toolchain}"]
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version=1", *flags], cwd=ROOT))
    selected = published_packages(metadata)
    target = Path(metadata["target_directory"])
    arguments = ["cargo", "package", "--locked", "--no-verify", *flags]
    if allow_dirty:
        arguments.append("--allow-dirty")
    for name in RELEASE_ORDER:
        arguments += ["-p", name]
    run(arguments)
    # This target may be shared by sequential commands; no second build cache is retained.
    with tempfile.TemporaryDirectory(prefix="neocogi-package-check-") as directory:
        outside = Path(directory)
        packages = outside / "packages"
        packages.mkdir()
        members = []
        patches = {}
        for name, package in sorted(selected.items()):
            archive = target / "package" / f"{name}-{package['version']}.crate"
            prefix = inspect_archive(archive, package)
            with tarfile.open(archive) as bundle:
                bundle.extractall(packages, filter="data")
            members.append(f"packages/{prefix}")
            patches[name] = f"packages/{prefix}"
        shutil.copytree(ROOT / "scripts/package-consumer", outside / "consumer")
        members.append("consumer")
        manifest = "[workspace]\nresolver='3'\nmembers=" + json.dumps(members) + "\n\n[patch.crates-io]\n"
        for name, path in patches.items():
            manifest += f"{name}={{path={json.dumps(path)}}}\n"
        (outside / "Cargo.toml").write_text(manifest)
        shutil.copyfile(ROOT / "Cargo.lock", outside / "Cargo.lock")
        common = [*flags, "--target-dir", str(target)]
        # Cargo archives have normalized old mtimes. In a reused target, artifacts
        # from an earlier extraction of this same version must not certify changed
        # archive bytes. Clean only this release's packages, retaining third-party
        # dependency builds and the one shared target directory.
        clean = [*cargo, "clean", *flags]
        for name in sorted(selected):
            clean += ["-p", name]
        run(clean)
        # Package manifests and build inputs are unmodified. Patches resolve only
        # unpublished internal dependencies to the exact extracted archive contents.
        run([*cargo, "build", "--workspace", "--all-targets", *common], cwd=outside)
        run([*cargo, "test", "-p", CONTRACTS_OWNER, "--features", "build-support,test-support", *common], cwd=outside)
        run([*cargo, "test", "--lib", "--test", "compiler", "--test", "shader_targets", "-p", "neocogi-platform", "--features", "test-support,platform,glow,shader-spirv", *common], cwd=outside)
        run([*cargo, "run", "-p", "neocogi-package-consumer", *common], cwd=outside)
        # This exercises real Cargo invalidation after replacing the staging
        # proc macros: missing outputs must restore, and changed authored bytes
        # must reach both the rebuilt executable and the deployed snapshot.
        staged = outside / "consumer/staged/asset.txt"
        staged.unlink()
        run([*cargo, "run", "-p", "neocogi-package-consumer", *common], cwd=outside)
        (outside / "consumer/asset.txt").write_bytes(b"changed packaged staging input\n")
        run([*cargo, "run", "-p", "neocogi-package-consumer", *common], cwd=outside)
        if stable:
            run(["cargo", f"+{stable}", "run", "-p", "neocogi-package-consumer", *common], cwd=outside)
    print(f"PASS: {len(selected)} archives and external consumer; internal sources came only from extracted archives", flush=True)


def main():
    """Expose explicit network, dirty-tree and optional stable-consumer choices."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--stable-toolchain", help="also run the CPU consumer using this installed stable toolchain")
    options = parser.parse_args()
    qualify(allow_dirty=options.allow_dirty, offline=options.offline, stable=options.stable_toolchain)


if __name__ == "__main__":
    main()
