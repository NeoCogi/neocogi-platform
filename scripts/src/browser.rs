//! Build the real Wasm artifact, serve its web bindings and require an exact completed browser result.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

/// Only compiler artifacts carry filenames; other Cargo JSON messages are deliberately ignored.
#[derive(Deserialize)]
#[serde(tag = "reason")]
enum CargoMessage {
    /// Emitted output from one actual compiled Cargo target.
    #[serde(rename = "compiler-artifact")]
    Artifact {
        /// Target identity, independent of output layout.
        target: Target,
        /// Concrete emitted paths reported by Cargo.
        filenames: Vec<PathBuf>,
    },
    /// Diagnostics, build-script events and final completion contain no artifact to select.
    #[serde(other)]
    Other,
}

/// Minimal target metadata needed to distinguish the browser fixture from dependencies.
#[derive(Deserialize)]
struct Target {
    /// Rust target name reported by Cargo.
    name: String,
}

/// Read the result node through an HTML parser; source text and duplicate results cannot report success.
fn require_pass(document: &str) -> Result<()> {
    let document = scraper::Html::parse_document(document);
    let selector = scraper::Selector::parse("pre#result").expect("literal CSS selector");
    let results: Vec<_> = document.select(&selector).collect();
    ensure!(
        results.len() == 1,
        "Browser regression did not pass: expected one result element, found {}",
        results.len()
    );
    let result = results[0].text().collect::<String>();
    ensure!(result.trim() == "PASS", "Browser regression did not pass: {}", result.trim());
    Ok(())
}

/// A failed Cargo build never yields success even if earlier JSON contained a Wasm filename.
fn build_artifact() -> Result<PathBuf> {
    let bytes = crate::process::capture(crate::process::cargo(None).args([
        "build",
        "--locked",
        "-p",
        "neocogi-browser-tests",
        "--target",
        "wasm32-unknown-unknown",
        "--message-format=json-render-diagnostics",
    ]))?;
    artifact(&bytes)
}

/// Discover the fixture's emitted Wasm path rather than assuming Cargo's target directory layout.
fn artifact(bytes: &[u8]) -> Result<PathBuf> {
    for line in bytes.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        if let CargoMessage::Artifact { target, filenames } = serde_json::from_slice(line)?
            && target.name == "neocogi_browser_tests"
            && let Some(path) = filenames.into_iter().find(|path| path.extension().is_some_and(|extension| extension == "wasm"))
        {
            return Ok(path);
        }
    }
    anyhow::bail!("Cargo did not report the browser regression Wasm artifact")
}

/// Find explicit tools unchanged, otherwise search PATH including Windows executable extensions.
fn tool(explicit: Option<PathBuf>, names: &[&str], option: &str) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    for name in names {
        for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            let path = directory.join(name);
            if path.is_file() {
                return Ok(path);
            }
            if cfg!(windows) && path.with_extension("exe").is_file() {
                return Ok(path.with_extension("exe"));
            }
        }
    }
    anyhow::bail!("{} is required; supply --{option}", names.join("/"))
}

/// One loopback HTTP server owns a worker and always shuts it down before temporary files are removed.
struct WebServer {
    /// Bound loopback address with an OS-selected ephemeral port.
    address: SocketAddr,
    /// Explicit stop signal, separate from successful or failed browser execution.
    stop: mpsc::Sender<()>,
    /// Joined during Drop so no request can outlive its fixture directory.
    worker: Option<thread::JoinHandle<()>>,
}

/// One fixture's load-event barrier; the pending iframe keeps DOM capture behind actual asynchronous completion.
enum CompletionGate {
    /// The fixture's initial pending request has not reached the server.
    Idle,
    /// Own the response until the browser explicitly completes all synchronous/asynchronous assertions.
    Waiting(tiny_http::Request),
    /// Completion can race ahead of the initial request; later requests finish immediately.
    Complete,
}
impl CompletionGate {
    /// Handle only the fixture's two control routes, leaving static asset requests responsive.
    fn handle(&mut self, request: tiny_http::Request) -> Option<tiny_http::Request> {
        match (request.url(), request.method()) {
            ("/test-pending", tiny_http::Method::Get) => {
                match self {
                    Self::Idle => *self = Self::Waiting(request),
                    Self::Waiting(_) => {
                        let _ = request.respond(tiny_http::Response::empty(409));
                    }
                    Self::Complete => {
                        let _ = request.respond(tiny_http::Response::empty(204));
                    }
                }
                None
            }
            ("/test-complete", tiny_http::Method::Post) => {
                if let Self::Waiting(pending) = std::mem::replace(self, Self::Complete) {
                    let _ = pending.respond(tiny_http::Response::empty(204));
                }
                let _ = request.respond(tiny_http::Response::empty(204));
                None
            }
            _ => Some(request),
        }
    }
}

impl WebServer {
    /// Serve generated bindings only; browser profile data lives outside this root.
    fn start(root: PathBuf) -> Result<Self> {
        let server = tiny_http::Server::http(("127.0.0.1", 0)).map_err(|error| anyhow::anyhow!("start browser HTTP server: {error}"))?;
        let address = server.server_addr().to_ip().context("expected TCP listener")?;
        let (stop, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut gate = CompletionGate::Idle;
            while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                match server.recv_timeout(Duration::from_millis(50)) {
                    Ok(Some(request)) => {
                        if let Some(request) = gate.handle(request) {
                            serve(request, &root);
                        }
                    }
                    Ok(None) => (),
                    Err(_) => break,
                }
            }
        });
        Ok(Self { address, stop, worker: Some(worker) })
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Static GET/HEAD requests cannot escape the generated web directory; Wasm has its required MIME type.
fn serve(request: tiny_http::Request, root: &Path) {
    let name = request.url().split('?').next().unwrap_or("").trim_start_matches('/');
    let relative = Path::new(name);
    let path = root.join(relative);
    let valid =
        relative.components().all(|component| matches!(component, Component::Normal(_))) && path.canonicalize().is_ok_and(|path| path.starts_with(root));
    if !valid || !matches!(request.method(), tiny_http::Method::Get | tiny_http::Method::Head) {
        let _ = request.respond(tiny_http::Response::empty(404));
        return;
    }
    match fs::File::open(&path) {
        Ok(file) if path.is_file() => {
            let content_type = match path.extension().and_then(|extension| extension.to_str()) {
                Some("wasm") => "application/wasm",
                Some("js") => "text/javascript",
                Some("html") => "text/html; charset=utf-8",
                _ => "application/octet-stream",
            };
            let header = tiny_http::Header::from_bytes("Content-Type", content_type).expect("literal MIME type");
            let _ = request.respond(tiny_http::Response::from_file(file).with_header(header));
        }
        _ => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

/// Require matching binding tools, then execute the real DOM/WebGL fixture with disposable browser state.
pub(crate) fn run(browser: Option<PathBuf>, bindgen: Option<PathBuf>) -> Result<()> {
    let browser = tool(browser, &["chrome", "google-chrome", "chromium", "chromium-browser"], "browser")?;
    let bindgen = tool(bindgen, &["wasm-bindgen"], "wasm-bindgen")?;
    let version = crate::bindgen_version()?;
    let actual = crate::process::capture(Command::new(&bindgen).arg("--version"))?;
    ensure!(
        std::str::from_utf8(&actual)?.trim() == format!("wasm-bindgen {version}"),
        "Expected wasm-bindgen {version}, found {}",
        String::from_utf8_lossy(&actual).trim()
    );
    let artifact = build_artifact()?;
    let directory = tempfile::Builder::new().prefix("neocogi-browser-").tempdir()?;
    let web = directory.path().join("web");
    fs::create_dir(&web)?;
    crate::process::run(
        Command::new(bindgen)
            .arg(artifact)
            .args(["--target", "web", "--out-dir"])
            .arg(&web)
            .args(["--out-name", "input-test"]),
    )?;
    fs::copy(crate::root().join("crates/platform/tests/platform_browser-input.html"), web.join("index.html"))?;
    let server = WebServer::start(web.canonicalize()?)?;
    let output = crate::process::timed_output(
        Command::new(browser)
            .args([
                "--headless=new",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--disable-background-networking",
                "--no-first-run",
                "--no-default-browser-check",
                "--use-angle=swiftshader",
                "--enable-unsafe-swiftshader",
                "--use-fake-ui-for-media-stream",
                "--use-fake-device-for-media-stream",
                "--autoplay-policy=no-user-gesture-required",
            ])
            .arg(format!("--user-data-dir={}", directory.path().join("profile").display()))
            .args(["--dump-dom", "--timeout=80000"])
            .arg(format!("http://{}/index.html", server.address)),
        Duration::from_secs(90),
    )?;
    if !output.status.success() || require_pass(std::str::from_utf8(&output.stdout)?).is_err() {
        eprintln!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("Browser regression did not pass ({}); see captured DOM and diagnostics above", output.status);
    }
    println!("Browser platform DOM, WebGL and audio regressions: PASS");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tiny_http drops its accept listener on another thread. Observe actual TCP
    /// refusal under a finite deadline; an HTTP error alone cannot prove closure.
    fn assert_listener_stops(address: SocketAddr) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let attempt = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50));
            if matches!(&attempt, Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused) {
                return;
            }
            // Close successful probes immediately instead of keeping connection
            // readers alive while waiting for the library's listener to stop.
            drop(attempt);
            assert!(std::time::Instant::now() < deadline, "HTTP listener {address} did not stop");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// A source literal, missing/stuck result, duplicate result or explicit failure cannot acknowledge execution.
    #[test]
    fn exact_completed_result() {
        for document in ["<pre id='result'>PASS</pre>", "<PRE id=result> PASS </PRE>"] {
            require_pass(document).unwrap();
        }
        for document in [
            "<pre id='result'>RUNNING</pre>",
            "<pre id='result'>FAIL missing export</pre>",
            "<script>const result='PASS'</script>",
            "<pre id='other'>PASS</pre>",
            "<pre id='result'>PASS</pre><pre id='result'>FAIL later</pre>",
            "<pre id='result'>PASS</pre><pre id='result'></pre>",
        ] {
            assert!(require_pass(document).is_err(), "{document}");
        }
    }

    /// Cargo may put the fixture anywhere and emits unrelated artifacts and diagnostic records.
    #[test]
    fn discovers_actual_wasm_output() {
        let bytes = b"{\"reason\":\"build-finished\",\"success\":true}\n{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"dependency\"},\"filenames\":[\"wrong.wasm\"]}\n{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"neocogi_browser_tests\"},\"filenames\":[\"custom/path with spaces/input.wasm\"]}\n";
        assert_eq!(artifact(bytes).unwrap(), PathBuf::from("custom/path with spaces/input.wasm"));
        assert!(artifact(b"{\"reason\":\"build-finished\",\"success\":true}\n").is_err());
        assert!(artifact(b"invalid JSON").is_err());
    }

    /// Actual loopback serving gives Wasm the right content type and rejects paths outside the fixture.
    #[test]
    fn serves_wasm_and_stops_cleanly() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.wasm"), b"wasm").unwrap();
        let server = WebServer::start(directory.path().canonicalize().unwrap()).unwrap();
        let url = format!("http://{}/test.wasm", server.address);
        let mut response = ureq::get(&url).call().unwrap();
        assert_eq!(response.headers()["content-type"], "application/wasm");
        assert_eq!(response.body_mut().read_to_vec().unwrap(), b"wasm");
        assert!(ureq::get(format!("http://{}/../Cargo.toml", server.address)).call().is_err());
        let address = server.address;
        drop(server);
        assert_listener_stops(address);
    }

    /// Shutdown also closes listeners whose accept thread may not have run yet.
    #[test]
    fn unused_servers_release_their_listeners() {
        let directory = tempfile::tempdir().unwrap();
        for _ in 0..8 {
            let server = WebServer::start(directory.path().canonicalize().unwrap()).unwrap();
            let address = server.address;
            drop(server);
            assert_listener_stops(address);
        }
    }

    /// An outstanding completion barrier must not block modules/Wasm, and explicit completion releases it.
    #[test]
    fn asynchronous_completion_barrier_keeps_assets_responsive() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("module.js"), "export const ready = true;").unwrap();
        let server = WebServer::start(directory.path().canonicalize().unwrap()).unwrap();
        let address = server.address;
        let pending = thread::spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(3))).build().into();
            agent.get(format!("http://{address}/test-pending")).call().unwrap().status().as_u16()
        });
        assert!(ureq::get(format!("http://{address}/module.js")).call().unwrap().status().is_success());
        assert_eq!(
            ureq::post(format!("http://{address}/test-complete")).send_empty().unwrap().status().as_u16(),
            204
        );
        assert_eq!(pending.join().unwrap(), 204);
        drop(server);
        assert_listener_stops(address);
    }
}
