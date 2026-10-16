"""Validate Cargo source-map metadata boundaries without requiring a debugger."""
import json
from pathlib import Path
import tempfile
import unittest

from lldb_source_map import source_map


class CargoSourceMappings(unittest.TestCase):
    """Local source paths are read from Cargo metadata, without prefix guessing."""

    def test_actual_schema_and_untrimmed_target(self):
        """Preserve mapping direction and unusual paths; tolerate absent sidecars."""
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "app with spaces"
            self.assertEqual(source_map(executable), [])
            mappings = [(".", "/source with spaces/author's checkout"), ("/cargo/build-dir", str(Path(directory) / "custom target"))]
            sidecar = executable.with_name(executable.name + ".trim-paths.json")
            sidecar.write_text(json.dumps({"v": 1, "remaps": [{"from": a, "to": b} for a, b in mappings]}))
            self.assertEqual(source_map(executable), mappings)

    def test_rejects_corrupt_or_unsupported_metadata(self):
        """Unknown versions, malformed JSON and invalid pairs cannot become commands."""
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "app"
            sidecar = executable.with_name(executable.name + ".trim-paths.json")
            for contents in ("{", "[]", '{"v":2,"remaps":[]}', '{"v":1}',
                             '{"v":1,"remaps":[{"from":"."}]}',
                             '{"v":1,"remaps":[{"from":".","to":false}]}'):
                with self.subTest(contents=contents):
                    sidecar.write_text(contents)
                    with self.assertRaises(ValueError):
                        source_map(executable)


if __name__ == "__main__":
    unittest.main()
