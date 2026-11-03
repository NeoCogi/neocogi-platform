"""Failure detection for the headless browser runner, independent of installed browsers."""
import unittest
from test_browser import require_pass


class BrowserResultTests(unittest.TestCase):
    """Only the actual completed result element can acknowledge successful execution."""

    def test_exact_result_is_required(self):
        require_pass('<pre id="result">PASS</pre>')
        for html in [
            '<pre id="result">RUNNING</pre>',
            '<pre id="result">FAIL missing export</pre>',
            '<script>const result="PASS"</script>',
            '<pre id="other">PASS</pre>',
            '<pre id="result">PASS</pre><pre id="result">FAIL later error</pre>',
        ]:
            with self.subTest(html=html), self.assertRaises(RuntimeError):
                require_pass(html)
