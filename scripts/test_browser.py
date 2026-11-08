#!/usr/bin/env python3
"""Build the real browser regression artifact and require PASS from headless Chrome."""

import argparse
from functools import partial
from html.parser import HTMLParser
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import tomllib


ROOT = Path(__file__).resolve().parents[1]


class ResultParser(HTMLParser):
    """Read only the result element, avoiding PASS text from source code or other nodes."""

    def __init__(self):
        super().__init__()
        self.active = False
        self.result = []

    def handle_starttag(self, tag, attrs):
        if tag == "pre" and dict(attrs).get("id") == "result":
            self.active = True

    def handle_endtag(self, tag):
        if tag == "pre":
            self.active = False

    def handle_data(self, data):
        if self.active:
            self.result.append(data)


def require_pass(document: str) -> None:
    """Missing output, a stuck RUNNING state, and every explicit failure reject."""
    parser = ResultParser()
    parser.feed(document)
    result = "".join(parser.result).strip()
    if result != "PASS":
        raise RuntimeError(f"Browser regression did not pass: {result or 'missing result element'}")


def build_artifact() -> Path:
    """Use Cargo's actual artifact JSON rather than assuming its output layout."""
    result = subprocess.run(
        ["cargo", "build", "--locked", "-p", "neocogi-browser-tests",
         "--target", "wasm32-unknown-unknown", "--message-format=json-render-diagnostics"],
        cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True,
    )
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message["target"]["name"] == "neocogi_browser_tests":
            for filename in message["filenames"]:
                if filename.endswith(".wasm"):
                    return Path(filename)
    raise RuntimeError("Cargo did not report the browser regression Wasm artifact")


def tool(name: str, explicit: str | None) -> str:
    """Resolve an explicitly supplied tool or one installed on PATH."""
    resolved = explicit or shutil.which(name)
    if not resolved:
        raise RuntimeError(f"{name} is required; supply --{name}")
    return resolved


def run(browser: str, bindgen: str) -> None:
    """Generate matching bindings and execute installed Rust listeners and GPU tests."""
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["dependencies"]["wasm-bindgen"].lstrip("=")
    actual = subprocess.check_output([bindgen, "--version"], text=True).strip()
    if actual != f"wasm-bindgen {version}":
        raise RuntimeError(f"Expected wasm-bindgen {version}, found {actual}")
    artifact = build_artifact()
    with tempfile.TemporaryDirectory(prefix="neocogi-browser-") as directory:
        directory = Path(directory)
        subprocess.run([bindgen, str(artifact), "--target", "web", "--out-dir", str(directory),
                        "--out-name", "input-test"], check=True)
        shutil.copyfile(ROOT / "crates/platform/tests/platform_browser-input.html", directory / "index.html")
        handler = partial(SimpleHTTPRequestHandler, directory=str(directory))
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}/index.html"
            result = subprocess.run(
                [browser, "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
                 "--disable-background-networking", "--no-first-run", "--no-default-browser-check",
                 "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
                 f"--user-data-dir={directory / 'profile'}",
                 "--dump-dom", "--virtual-time-budget=60000", url],
                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=90, check=True,
            )
            try:
                require_pass(result.stdout)
            except RuntimeError:
                print(result.stdout)
                print(result.stderr[-8000:])
                raise
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
    print("Browser platform DOM and WebGL regressions: PASS")


def main() -> None:
    """Select external tool paths; browser state and generated bindings are disposable."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", help="Chrome/Chromium executable")
    parser.add_argument("--wasm-bindgen", help="CLI matching the workspace wasm-bindgen version")
    args = parser.parse_args()
    browser = args.browser or next((shutil.which(name) for name in
                                   ["chrome", "google-chrome", "chromium", "chromium-browser"]
                                   if shutil.which(name)), None)
    if not browser:
        parser.error("Chrome/Chromium is required; supply --browser")
    run(browser, tool("wasm-bindgen", args.wasm_bindgen))


if __name__ == "__main__":
    main()
