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
            (path / "Cannon_Exterior.hdr").write_bytes(b"valid")
            digest = hashlib.sha256(b"valid").hexdigest()
            with patch.object(fetcher, "MODELS", {"Model": digest}), patch.object(fetcher, "ENVIRONMENT_SHA256", digest), patch.object(fetcher.urllib.request, "urlopen") as request:
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
                        fetcher.verified_download(model, "https://example.test/model", hashlib.sha256(b"valid").hexdigest())
                self.assertEqual(model.read_bytes(), b"old")
                self.assertFalse(model.with_suffix(".glb.part").exists())
                with patch.object(fetcher.urllib.request, "urlopen", return_value=io.BytesIO(b"valid")):
                    fetcher.verified_download(model, "https://example.test/model", hashlib.sha256(b"valid").hexdigest())
                self.assertEqual(model.read_bytes(), b"valid")
                self.assertFalse(model.with_suffix(".glb.part").exists())

    def test_hdr_is_fetched_from_its_pinned_repository(self):
        """The external environment has its own revision and integrity check."""
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(fetcher, "verified_download") as download:
                fetcher.fetch(Path(directory))
            path, url, digest = download.call_args.args
            self.assertEqual(path.name, "Cannon_Exterior.hdr")
            self.assertIn(fetcher.ENVIRONMENT_REVISION, url)
            self.assertIn("glTF-Sample-Environments", url)
            self.assertEqual(digest, fetcher.ENVIRONMENT_SHA256)


if __name__ == "__main__":
    unittest.main()
