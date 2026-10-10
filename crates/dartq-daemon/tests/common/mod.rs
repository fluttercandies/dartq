//! Shared test fixture: a throwaway pub workspace + synthetic `dart:core`
//! VM-segment SDK, assembled through the real dartq-pub/dartq-package/
//! dartq-sdk faces (the same assembly the CLI's `open_engine` performs),
//! plus a scripted in-memory-pipe client that paces the server one turn at
//! a time.

#![allow(dead_code)]

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dartq_daemon::{Session, run};
use dartq_engine::{EngineConfig, memory_store};
use dartq_package::PackageTable;
use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
use dartq_sdk::SdkSnapshot;
use dartq_snapshot::{DEFAULT_CAPACITY_BYTES, SnapshotConfig, SnapshotStore};
use serde_json::{Value, json};

/// How long a paced step may wait for the server to settle.
const SETTLE: Duration = Duration::from_secs(10);

/// Seeded disk content: one resolvable URI-layer error (`uri_does_not_exist`),
/// deterministic under the synthetic SDK.
pub const BROKEN_MAIN: &str = "import 'missing.dart';\nvoid main() {}\n";
/// Clean replacement pushed through didChange.
pub const CLEAN_MAIN: &str = "void main() {}\n";

/// A throwaway single-package workspace with `lib/main.dart` on disk.
pub struct Workspace {
    pub root: PathBuf,
    pub config: Arc<PackageConfig>,
    pub config_dir: PathBuf,
    pub packages: PackageTable,
}

impl Workspace {
    #[must_use]
    pub fn new(main_dart: &str) -> Self {
        // A unique tree per instance: session tests run in parallel, and a
        // shared pid-scoped path would have each setup's remove_dir_all
        // tear down another test's files mid-read.
        static WORKSPACE_SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-daemon-test-{}-{}",
            std::process::id(),
            WORKSPACE_SEQ.fetch_add(1, Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        );
        write("sdk/version", "3.13.1\n");
        write(
            "sdk/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdk/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write("lib/main.dart", main_dart);
        // The conventional package layout includes the test directory even
        // when the seeded files don't touch it: fixtures that add test
        // files write into it directly.
        std::fs::create_dir_all(root.join("test")).expect("test dir");

        // macOS /var → /private/var: the engine's package table canonicalizes
        // its config dir, so every face and every test assertion must derive
        // from the canonical root or the URIs disagree.
        let root = root.canonicalize().expect("root canonicalizes");
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(
            PackageConfig::parse(
                &std::fs::read_to_string(config_dir.join("package_config.json")).expect("config"),
            )
            .expect("valid config"),
        );
        let lock =
            LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n").expect("lock");
        let pubspec =
            Pubspec::parse("name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
                .expect("pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &pubspec,
            &std::collections::BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");
        Self {
            root,
            config,
            config_dir,
            packages,
        }
    }

    /// The workspace's canonical root.
    #[must_use]
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The root context configuration over the workspace (the same assembly
    /// the CLI's `open_engine` performs), for [`dartq_daemon::Session::new`].
    #[must_use]
    pub fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&self.root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: self.packages.clone(),
            pub_config: Arc::clone(&self.config),
            config_dir: self.config_dir.clone(),
            semdiag: Some(dartq_engine::semdiag::diagnoser()),
        }
    }
}

/// A throwaway snapshot-store root (removed first so assertions on its
/// contents start clean). The sequence suffix makes every call a unique
/// tree: two callers passing the same `name`, or a crashed prior run whose
/// pid was recycled, can never share state — the `name` stays in the path
/// only for diagnosability of leftovers.
#[must_use]
pub fn store_root(name: &str) -> PathBuf {
    static STORE_SEQ: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "dartq-daemon-snap-{}-{}-{name}",
        std::process::id(),
        STORE_SEQ.fetch_add(1, Relaxed),
    ));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// The client end of the transport: a channel of byte chunks read as a
/// stream. Disconnection is EOF.
struct ChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            self.pending = self.receiver.recv().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "client disconnected")
            })?;
        }
        let take = buf.len().min(self.pending.len());
        buf[..take].copy_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        Ok(take)
    }
}

impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.pending.is_empty() {
            self.pending = self.receiver.recv().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "client disconnected")
            })?;
        }
        Ok(&self.pending)
    }

    fn consume(&mut self, amount: usize) {
        self.pending.drain(..amount);
    }
}

/// The server's output side: everything the session writes, appended.
#[derive(Clone, Default)]
pub struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("output lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A running session: the server loop on its own thread, the test thread as
/// the paced client. Steps settle through transcript-frame counts; a step
/// that produces no frames settles through a probe `dartq/analyze` (its
/// answer proves the server turned past every earlier message, and it never
/// pushes).
pub struct Harness {
    client: mpsc::Sender<Vec<u8>>,
    output: SharedOutput,
    server: JoinHandle<std::io::Result<u8>>,
    probe: i64,
}

/// Spawns the server over `workspace` with the snapshot store pinned to
/// `store_root`.
#[must_use]
pub fn spawn(workspace: &Workspace, store_root: &Path) -> Harness {
    let (client, receiver) = mpsc::channel();
    let store = SnapshotStore::open(SnapshotConfig {
        root: Some(store_root.to_path_buf()),
        capacity_bytes: DEFAULT_CAPACITY_BYTES,
    })
    .expect("store opens on a temp root");
    let session = Session::with_store(workspace.engine_config(), store).expect("session");
    let output = SharedOutput::default();
    let reader = ChannelReader {
        receiver,
        pending: Vec::new(),
    };
    let server = {
        let output = output.clone();
        thread::Builder::new()
            .name("dartq-daemon-test-server".into())
            .spawn(move || run(std::io::BufReader::new(reader), output, session, None))
            .expect("server thread spawns")
    };
    Harness {
        client,
        output,
        server,
        probe: 1000,
    }
}

impl Harness {
    /// Sends one message and waits until the server has fully processed it:
    /// `expect` new transcript frames, or a probe answer for a frame-less
    /// step.
    pub fn step(&mut self, message: &Value, expect: usize) {
        let before = self.transcript().len();
        self.client.send(framed(message)).expect("client send");
        if expect == 0 {
            self.probe_step();
        } else {
            self.settle(before + expect);
        }
    }

    /// Sends messages without pacing: the server drains them as one batch
    /// (the writer-priority path). All frames travel as ONE byte chunk — a
    /// single channel send — so the reader decodes the whole batch off a
    /// single wake instead of re-blocking between frames.
    pub fn burst(&mut self, messages: &[Value], expect: usize) {
        let before = self.transcript().len();
        let mut chunk = Vec::new();
        for message in messages {
            chunk.extend_from_slice(&framed(message));
        }
        self.client.send(chunk).expect("client send");
        self.settle(before + expect);
    }

    /// Sends raw, unframed bytes (transport-fault tests).
    pub fn send_raw(&mut self, bytes: &[u8]) {
        self.client.send(bytes.to_vec()).expect("client send");
    }

    /// The full decoded transcript so far.
    #[must_use]
    pub fn transcript(&self) -> Vec<Value> {
        frames(&self.output.0.lock().expect("output lock"))
    }

    /// Sends `exit`, waits for the server loop to end, and returns the exit
    /// code with the full transcript.
    pub fn exit(self) -> (u8, Vec<Value>) {
        self.client
            .send(framed(&notification("exit", json!(null))))
            .expect("exit send");
        self.join_ended()
    }

    /// Waits for an already-ending server (desync or EOF) and returns the
    /// exit code with the full transcript.
    pub fn join_ended(self) -> (u8, Vec<Value>) {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline && !self.server.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            self.server.is_finished(),
            "server did not end; transcript: {:?}",
            self.transcript()
        );
        let transcript = self.transcript();
        let code = self
            .server
            .join()
            .expect("server thread does not panic")
            .expect("transport does not fail on memory pipes");
        (code, transcript)
    }

    /// Waits until the transcript holds `total` frames.
    fn settle(&self, total: usize) {
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            if self.transcript().len() >= total {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!(
            "expected {total} transcript frames, have {}; transcript: {:?}",
            self.transcript().len(),
            self.transcript()
        );
    }

    /// A frame-less step's sync: a `dartq/analyze` probe whose answer proves
    /// every earlier message was processed (in-order intake, and the drain
    /// executes writes before reads).
    fn probe_step(&mut self) {
        self.probe += 1;
        let id = self.probe;
        self.client
            .send(framed(&request(id, "dartq/analyze", json!({}))))
            .expect("probe send");
        let deadline = Instant::now() + SETTLE;
        while Instant::now() < deadline {
            let answered = self
                .transcript()
                .iter()
                .any(|frame| frame.get("id").is_some_and(|frame_id| frame_id == id));
            if answered {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!(
            "probe {id} never answered; transcript: {:?}",
            self.transcript()
        );
    }
}

/// Decodes the framed transcript back into JSON values.
#[must_use]
pub fn frames(bytes: &[u8]) -> Vec<Value> {
    let mut values = Vec::new();
    let mut rest: &str = std::str::from_utf8(bytes).expect("transcript is UTF-8");
    while let Some(position) = rest.find("Content-Length: ") {
        let after = &rest[position + "Content-Length: ".len()..];
        let line_end = after.find("\r\n").expect("header terminator");
        let length: usize = after[..line_end].parse().expect("Content-Length value");
        let body_start = line_end + "\r\n\r\n".len();
        let body = &after[body_start..body_start + length];
        values.push(serde_json::from_str(body).expect("frame body is JSON"));
        rest = &after[body_start + length..];
    }
    values
}

/// The transcript's request answers, client-issued ids only (probe ids run
/// from 1001 up).
#[must_use]
pub fn answered(transcript: &[Value]) -> Vec<&Value> {
    transcript
        .iter()
        .filter(|frame| {
            frame
                .get("id")
                .is_some_and(|id| id.as_i64().is_some_and(|n| n < 1000))
        })
        .collect()
}

/// The transcript's `textDocument/publishDiagnostics` pushes.
#[must_use]
pub fn publishes(transcript: &[Value]) -> Vec<&Value> {
    transcript
        .iter()
        .filter(|frame| frame["method"] == "textDocument/publishDiagnostics")
        .collect()
}

#[must_use]
pub fn framed(value: &Value) -> Vec<u8> {
    let body = serde_json::to_string(value).expect("serializes");
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

#[must_use]
pub fn request(id: i64, method: &str, params: impl Into<Value>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params.into()})
}

#[must_use]
pub fn notification(method: &str, params: impl Into<Value>) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params.into()})
}
