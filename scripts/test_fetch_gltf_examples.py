"""Offline checks for verified caching and atomic replacement of sample assets."""
import hashlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import fetch_gltf_examples as fetcher


class FetchTests(unittest.TestCase):
    """Exercise downloads without relying on a live upstream server."""

    def test_verified_cache_avoids_network(self):
        """An existing verified file must be sufficient for an offline invocation."""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "Model.glb").write_bytes(b"valid")
            with patch.object(fetcher, "MODELS", {"Model": hashlib.sha256(b"valid").hexdigest()}), patch.object(fetcher.urllib.request, "urlopen") as request:
                fetcher.fetch(path)
                request.assert_not_called()

    def test_download_verifies_before_replacing(self):
        """Corrupt downloads preserve existing content; valid downloads replace it."""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            model = path / "Model.glb"
            model.write_bytes(b"old")
            with patch.object(fetcher, "MODELS", {"Model": hashlib.sha256(b"valid").hexdigest()}):
                with patch.object(fetcher.urllib.request, "urlopen", return_value=io.BytesIO(b"corrupt")):
                    with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                        fetcher.fetch(path)
                self.assertEqual(model.read_bytes(), b"old")
                self.assertFalse(model.with_suffix(".glb.part").exists())
                with patch.object(fetcher.urllib.request, "urlopen", return_value=io.BytesIO(b"valid")):
                    fetcher.fetch(path)
                self.assertEqual(model.read_bytes(), b"valid")
                self.assertFalse(model.with_suffix(".glb.part").exists())


if __name__ == "__main__":
    unittest.main()
