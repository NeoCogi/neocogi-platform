"""Final-artifact selection and publication regressions independent of a GPU."""

from pathlib import Path
import os
import stat
import tempfile
import unittest

from build_plugins import install, shared_libraries


class PluginArtifacts(unittest.TestCase):
    """Select Cargo's exact final roots and preserve existing deployed files."""

    def artifact(self, name, path, kind="cdylib"):
        """Model one compiler-artifact message without depending on output hashes."""
        return {"reason": "compiler-artifact", "target": {"name": name, "crate_types": [kind]}, "filenames": [path]}

    def test_both_roots_are_required_and_rlibs_are_not_selected(self):
        """A dependency rlib or incomplete build cannot become a deployable plugin set."""
        platform = self.artifact("nc_platform", "/chosen/libnc_platform.so")
        compiler = self.artifact("nc_shader_compiler", "/chosen/examples/libnc_shader_compiler.so")
        ignored = self.artifact("nc_platform", "/wrong/libnc_platform.rlib", "rlib")
        self.assertEqual(shared_libraries([ignored, platform, compiler]), {
            "nc_platform": Path("/chosen/libnc_platform.so"),
            "nc_shader_compiler": Path("/chosen/examples/libnc_shader_compiler.so"),
        })
        with self.assertRaisesRegex(ValueError, "both final"):
            shared_libraries([platform])

    def test_unchanged_files_keep_timestamps_and_changed_bytes_replace(self):
        """Staging compares content and does not churn an unchanged deployment."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, destination = root / "library.so", root / "app/lib/plugin.so"
            source.write_bytes(b"first")
            install(source, destination)
            previous = destination.stat().st_mtime_ns
            install(source, destination)
            self.assertEqual(destination.stat().st_mtime_ns, previous)
            source.write_bytes(b"other")
            install(source, destination)
            self.assertEqual(destination.read_bytes(), b"other")
            if os.name != "nt":
                source.chmod(0o640)
                install(source, destination)
                self.assertEqual(stat.S_IMODE(destination.stat().st_mode), 0o640)


if __name__ == "__main__":
    unittest.main()
