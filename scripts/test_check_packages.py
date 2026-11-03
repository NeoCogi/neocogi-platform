"""Reject incomplete, unsafe or wrongly owned package inputs before consumer builds."""

import io
from pathlib import Path
import tarfile
import tempfile
import unittest

from check_packages import SCHEMA_OWNER, inspect_archive, published_packages


class ArchiveChecks(unittest.TestCase):
    """Small adversarial archives isolate checks from network and Cargo execution."""

    def archive(self, directory, *, owner="library", extra=None, omit=(), dependency=""):
        """Construct an explicit versioned archive with controllable malformed members."""
        package = {"name": owner, "version": "0.1.0-alpha.1"}
        prefix = f"{owner}-{package['version']}"
        files = {
            "Cargo.toml": f'[package]\nname="{owner}"\nversion="{package["version"]}"\n{dependency}',
            "README.md": "Package documentation",
            "LICENSE": "License notice",
        }
        if owner == SCHEMA_OWNER:
            files.update({f"schemas/{index}.ncidl": "declaration" for index in range(9)})
        files.update(extra or {})
        output = Path(directory) / "fixture.crate"
        with tarfile.open(output, "w:gz") as bundle:
            for name, content in files.items():
                if name in omit:
                    continue
                member = tarfile.TarInfo(f"{prefix}/{name}")
                data = content.encode()
                member.size = len(data)
                bundle.addfile(member, io.BytesIO(data))
        return output, package

    def test_archive_accepts_normalized_registry_inputs(self):
        """Ordinary files plus declared registry dependencies are sufficient for inspection."""
        with tempfile.TemporaryDirectory() as directory:
            path, package = self.archive(directory, dependency='[dependencies.helper]\nversion="=0.1.0-alpha.1"\n')
            self.assertEqual(inspect_archive(path, package), "library-0.1.0-alpha.1")

    def test_missing_documentation_or_license_rejects(self):
        """A successful Cargo archive creation must not hide missing release notices."""
        with tempfile.TemporaryDirectory() as directory:
            for name in ["README.md", "LICENSE", "Cargo.toml"]:
                with self.subTest(name=name):
                    path, package = self.archive(directory, omit=[name])
                    with self.assertRaisesRegex(ValueError, "missing"):
                        inspect_archive(path, package)

    def test_repository_dependencies_reject_in_target_tables_too(self):
        """Target-specific paths cannot bypass the external-dependency qualification."""
        with tempfile.TemporaryDirectory() as directory:
            for group in ["dependencies", "build-dependencies", "dev-dependencies", "target.'cfg(unix)'.dependencies"]:
                path, package = self.archive(directory, dependency=f'[{group}.helper]\npath="../checkout"\n')
                with self.subTest(group=group), self.assertRaisesRegex(ValueError, "nonregistry"):
                    inspect_archive(path, package)

    def test_unsafe_paths_and_local_review_notes_reject(self):
        """An archive cannot escape its root or accidentally distribute review notes."""
        with tempfile.TemporaryDirectory() as directory:
            for name in ["../../escape", "OPEN_ITEMS.md", "ADVERSARIAL_REVIEW.md"]:
                path, package = self.archive(directory, extra={name: "bad"})
                with self.subTest(name=name), self.assertRaises(ValueError):
                    inspect_archive(path, package)

    def test_sources_have_one_owner(self):
        """Other packages may not duplicate the authored declarations."""
        with tempfile.TemporaryDirectory() as directory:
            path, package = self.archive(directory, extra={"schemas/copy.ncidl": "duplicate"})
            with self.assertRaisesRegex(ValueError, "source ownership"):
                inspect_archive(path, package)
            path, package = self.archive(directory, owner=SCHEMA_OWNER)
            inspect_archive(path, package)
            path, package = self.archive(directory, owner=SCHEMA_OWNER, omit=["schemas/0.ncidl"])
            with self.assertRaisesRegex(ValueError, "source ownership"):
                inspect_archive(path, package)

    def test_private_publication_and_mixed_versions_reject(self):
        """Metadata policy is checked before Cargo constructs release archives."""
        packages = [
            {"id": "build", "name": SCHEMA_OWNER, "version": "0.1.0-alpha.1", "publish": ["crates-io"]},
            {"id": "examples", "name": "neocogi-platform-examples", "version": "0.1.0-alpha.1", "publish": []},
        ]
        metadata = {"workspace_members": [p["id"] for p in packages], "packages": packages}
        self.assertEqual(set(published_packages(metadata)), {SCHEMA_OWNER})
        packages[1]["publish"] = ["crates-io"]
        with self.assertRaisesRegex(ValueError, "private package"):
            published_packages(metadata)
        packages[1].update(name="other-library", version="0.1.0-alpha.2")
        with self.assertRaisesRegex(ValueError, "inconsistent versions"):
            published_packages(metadata)


if __name__ == "__main__":
    unittest.main()
