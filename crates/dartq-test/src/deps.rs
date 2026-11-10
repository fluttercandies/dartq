//! The directive URIs one Dart file pulls in — the edges of the import
//! graph the result cache keys suites on.
//!
//! Extraction runs on the dartq scanner's token table (the official
//! lexical grammar), never on regexes: comments, string bodies and
//! interpolation are already resolved by the scanner, so a commented-out
//! `import` or an `import` spelled inside a string is never mistaken for a
//! directive.
//!
//! Anything the extractor cannot read with certainty — an interpolated or
//! escaped directive URI, a directive that never terminates, a non-UTF-8
//! file — makes the file *opaque*, and a suite reaching an opaque file is
//! never cached.
//!
//! Beyond the edges, the same pass records what the file may read at run
//! time outside its imports: the process location (cwd-relative reads) and
//! literal paths. The cache widens a suite's key with those inputs, and
//! pins a suite whose closure observes the checkout's absolute location to
//! that location.

use dartq_scanner::{Keyword, ScanConfig, TokenKind, TokenTable};
use dartq_source::FileTable;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What one Dart file contributes to the import graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFacts {
    /// `import`/`export`/`part` URIs in source order, every conditional
    /// alternative included (the cache cannot know which one the
    /// toolchain picks, so all of them are inputs).
    pub uris: Vec<String>,
    /// The directive header could not be read with certainty.
    pub opaque: bool,
    /// Names the process location or spawns processes (`Directory.current`,
    /// package:path's `current`, `Platform.script`, `Uri.base`,
    /// `Isolate.packageConfig`, `Process.*`): it may read any file of the
    /// package.
    pub ambient: bool,
    /// Derives an absolute location: everything [`Self::ambient`] covers,
    /// plus `.absolute`, `resolveSymbolicLinks`, package:path's `absolute`/
    /// `relative`/`canonicalize`, and `StackTrace.current`. Such a value
    /// differs between two checkouts of the same content, and a test can
    /// observe the difference in ways no static rule bounds (its length,
    /// its depth, a substring), so a suite reaching one is keyed on its
    /// checkout's absolute root unless the file attests otherwise
    /// ([`Self::portable`]).
    pub located: bool,
    /// Absolute file paths spelled in string literals (`'/etc/hosts'`, a
    /// `file:` URI's path), normalized and deduplicated. Each is an input
    /// of every suite reaching the file — its bytes, or its absence — so a
    /// mock path that names nothing on disk keys as absent, and a real one
    /// keys on its content. A literal naming an existing directory (whose
    /// listing no key covers) makes the suite uncacheable.
    pub absolute_paths: Vec<String>,
    /// Carries the `// dartq: portable` attestation: the absolute locations
    /// this file derives reach no verdict except relative to the package
    /// root, so the outcome is the same in every checkout of the content.
    pub portable: bool,
    /// Observes how the test process itself was launched
    /// (`Platform.script`, `Platform.executableArguments`): the value
    /// differs between the stock runner, which spawns each suite from its
    /// own bootstrap kernel, and dartq's shared isolate group, so such a
    /// suite always runs under the stock loader.
    pub launch: bool,
    /// Carries the `// dartq: deterministic` attestation: the author vouches
    /// that this file's drifting, HTTP and wall-clock reads never reach a
    /// test verdict (a log timestamp, an analyzer modification stamp).
    /// Only this file's own reads are vouched for.
    pub attested: bool,
    /// String literals (or literal prefixes of interpolations) shaped like
    /// a relative path: `'lib'`, `'test/resources/x.dart'`, `'.'`.
    pub path_literals: Vec<String>,
    /// Touches state every engine of one `flutter_tester` process shares,
    /// which the stock tool gives each suite afresh: the font collection
    /// (`FontLoader`, `loadFontFromList`), the isolate name server, native
    /// memory and native globals (`dart:ffi`, `package:ffi`), or the working
    /// directory (`Directory.current =`). A suite reaching one never shares
    /// its tester with another suite. (No serde default: a stamp index
    /// written before this fact existed fails to load and is rebuilt,
    /// rather than vouching `false` for files it never examined.)
    pub engine_shared: bool,
    /// Carries the `// dartq: shardable` attestation: the author vouches
    /// that this suite's verdict is unchanged when its top-level entries
    /// run partitioned across two engines of one chunk, i.e. that (a) its
    /// suite-level `setUpAll`/`tearDownAll` (and its
    /// `flutter_test_config.dart` wrapper) may run twice in the checkout
    /// concurrently, and (b) no ordering or shared statics tie one
    /// top-level entry to another. Unlike [`Self::attested`], which vouches
    /// for a file's reads wherever the suite reaches it, this marker counts
    /// only in the suite's own file — it attests the suite's declaration
    /// structure and fixtures, so a marker inside a helper means nothing.
    /// (No serde default, like `engine_shared`.)
    pub shardable: bool,
    /// Environment variable names this file reads by literal key:
    /// `Platform.environment['NAME']` and
    /// `Platform.environment.containsKey('NAME')`, deduplicated in source
    /// order. Only these variables' values enter a suite's cache key — a
    /// terminal's ambient variables no longer invalidate replay. (No serde
    /// default, like `engine_shared`: a pre-existing stamp index fails to
    /// load and is rebuilt rather than vouching an empty read set.)
    pub env_reads: Vec<String>,
    /// The file's top-level `main` installs a flutter test binding
    /// ([`TEST_BINDINGS`]`.ensureInitialized()`, or the first
    /// `testWidgets(…)` declaration) before running any code of its own:
    /// everything it runs first is the declaration skeleton (`group`,
    /// `test`, `setUp`… calls whose arguments are literals and closures).
    /// The binding replaces dart:io's `HttpClient` factory with a client
    /// answering every request with an empty 400, and every closure the
    /// skeleton declares runs after it, so no HTTP client the suite builds
    /// reaches the network (see [`TriggerKind::Http`]). Only the suite's
    /// own file is consulted. (No serde default, like `engine_shared`.)
    pub test_binding: bool,
    /// Names what can take the binding's mock away: `HttpOverrides` (a
    /// client factory of its own), `overrideHttpClient` (a binding
    /// subclass opting out), `IntegrationTestWidgetsFlutterBinding` (the
    /// binding that keeps real networking), or a new isolate (`compute(…)`,
    /// `Isolate.spawn`/`spawnUri`/`run` — the binding's override is
    /// per-isolate). A suite whose closure carries one keeps no HTTP
    /// exemption. (No serde default, like `engine_shared`.)
    pub http_overrides: bool,
    /// The reads and markers that can refuse a suite reaching this file,
    /// one per label at its first line, in line order. They are both the
    /// verdict ([`Self::nondeterministic`], [`Self::time_read`],
    /// [`Self::http_read`], [`Self::no_cache`], [`Self::env_opaque`]) and
    /// the evidence a refused suite reports. Stationary randomness
    /// (`Random`, `Stopwatch` durations, `pid`) is never one: a replayed
    /// outcome is then one sample of the same distribution a fresh run
    /// draws from. (No serde default, like `engine_shared`: a stamp index
    /// written before this shape fails to load and is rebuilt.)
    ///
    /// Dot-shorthand `.environment['X']` accesses (Dart 3.10's inferred
    /// static invocation, no `Platform` receiver spelled) are not tracked;
    /// a suite relying on one opts out with `// dartq: no-cache`.
    pub triggers: Vec<Trigger>,
}

/// One read or marker that can refuse a suite (see [`FileFacts::triggers`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub kind: TriggerKind,
    /// What matched, as spelled: `Process.run`, `HttpClient()`,
    /// `DateTime.now`, `import package:http/http.dart`, `// dartq: no-cache`.
    pub label: String,
    /// 1-based line of the first match.
    pub line: u32,
}

/// What a [`Trigger`] reads, which decides who may vouch for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerKind {
    /// State that drifts between runs with no file, argument or
    /// environment change, and that no seam covers: sockets and network
    /// lookups (`Socket`, `ServerSocket`, `RawSocket`, `SecureSocket`,
    /// `RawDatagramSocket`, `WebSocket`, `HttpServer`, `InternetAddress`,
    /// `NetworkInterface`), child processes (`Process.run`/`runSync`/
    /// `start`), standard input, the host name, native code (`dart:ffi`),
    /// or a package whose purpose is one of those ([`DRIFT_PACKAGES`]).
    /// Refuses wherever the closure reaches it, unless the file attests
    /// `// dartq: deterministic`.
    Drift,
    /// An HTTP client: `HttpClient()`, `package:http`, `package:dio`. On
    /// the VM every request of these goes through dart:io's `HttpClient`
    /// factory, which the flutter test binding overrides with a client
    /// answering every request with an empty 400 — so a flutter suite that
    /// installs the binding ([`FileFacts::test_binding`]) observes a fixed
    /// response, and only a suite without it refuses.
    Http,
    /// The wall clock: `DateTime.now`/`timestamp`, `package:clock` (whose
    /// `clock` reads the zone's clock, the real one by default). Library
    /// code reads it on its caller's behalf (`DatePicker(initialDate: …)`,
    /// `withClock`/`fakeAsync` swap it), so only suite-side files refuse.
    Clock,
    /// The process environment read through a shape no literal name
    /// covers — iterating `Platform.environment`, a computed key, an alias
    /// of the map. What a suite reaching one does depends on the reading
    /// file's package tier: the suite's own package refuses caching, a
    /// dependency package folds the whole environment's digest into the
    /// key, and the toolchain tier (a first-party flutter binding aliasing
    /// the whole map as its default) is exempt.
    EnvOpaque,
    /// The `// dartq: no-cache` opt-out marker: the suite reaching this
    /// file always runs.
    NoCache,
}

impl FileFacts {
    fn has(&self, kind: TriggerKind) -> bool {
        self.triggers.iter().any(|trigger| trigger.kind == kind)
    }

    /// The first [`Trigger`] of `kind`, in line order.
    #[must_use]
    pub fn first(&self, kind: TriggerKind) -> Option<&Trigger> {
        self.triggers.iter().find(|trigger| trigger.kind == kind)
    }

    /// Reads drifting state no seam covers ([`TriggerKind::Drift`]).
    #[must_use]
    pub fn nondeterministic(&self) -> bool {
        self.has(TriggerKind::Drift)
    }

    /// Reads the wall clock ([`TriggerKind::Clock`]).
    #[must_use]
    pub fn time_read(&self) -> bool {
        self.has(TriggerKind::Clock)
    }

    /// Builds an HTTP client ([`TriggerKind::Http`]).
    #[must_use]
    pub fn http_read(&self) -> bool {
        self.has(TriggerKind::Http)
    }

    /// Carries the `// dartq: no-cache` marker.
    #[must_use]
    pub fn no_cache(&self) -> bool {
        self.has(TriggerKind::NoCache)
    }

    /// Reads the environment through an unnamable shape
    /// ([`TriggerKind::EnvOpaque`]).
    #[must_use]
    pub fn env_opaque(&self) -> bool {
        self.has(TriggerKind::EnvOpaque)
    }
}

/// Longest literal still considered a candidate path.
const MAX_PATH_LITERAL: usize = 255;

/// Packages whose API is a drifting source no seam covers: WebSocket and
/// RPC clients, servers, raw HTTP/2 sockets, and process spawning.
/// Importing one marks the file [`TriggerKind::Drift`]: the drift happens
/// inside the dependency, where the cache does not look.
pub const DRIFT_PACKAGES: &[&str] = &[
    "grpc",
    "http2",
    "io",
    "process",
    "process_run",
    "shelf",
    "web_socket",
    "web_socket_channel",
];

/// Packages whose every request goes through dart:io's `HttpClient`
/// factory on the VM ([`TriggerKind::Http`]).
pub const HTTP_PACKAGES: &[&str] = &["dio", "http"];

/// The receivers whose `ensureInitialized` installs the flutter test
/// binding (and with it the mock `HttpClient` factory).
pub const TEST_BINDINGS: &[&str] = &[
    "TestWidgetsFlutterBinding",
    "AutomatedTestWidgetsFlutterBinding",
    "LiveTestWidgetsFlutterBinding",
];

/// Extract the facts of one Dart file's bytes. A leading BOM is skipped,
/// as the toolchain does.
#[must_use]
pub fn facts(bytes: &[u8]) -> FileFacts {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return FileFacts {
            opaque: true,
            ..FileFacts::default()
        };
    };
    let source = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut files = FileTable::new();
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: false,
        },
        files.intern(Path::new("source.dart")),
    );
    let lines = &scanned.line_index;
    let line = |offset: usize| lines.byte_to_line_col(offset as u32).0;
    let mut out = FileFacts::default();
    let mut uri_offsets = Vec::new();
    out.opaque =
        read_directives(source, &scanned.tokens, &mut out.uris, &mut uri_offsets).is_none();
    read_runtime_inputs(source, &scanned.tokens, &line, &mut out);
    for (uri, &offset) in out.uris.iter().zip(&uri_offsets) {
        let package = uri
            .strip_prefix("package:")
            .and_then(|rest| rest.split_once('/'))
            .map(|(package, _)| package);
        let kind = match package {
            _ if uri == "dart:ffi" => TriggerKind::Drift,
            Some(package) if DRIFT_PACKAGES.contains(&package) => TriggerKind::Drift,
            Some(package) if HTTP_PACKAGES.contains(&package) => TriggerKind::Http,
            Some("clock") => TriggerKind::Clock,
            _ => continue,
        };
        out.triggers.push(Trigger {
            kind,
            label: format!("import {uri}"),
            line: line(offset),
        });
    }
    out.engine_shared |= out
        .uris
        .iter()
        .any(|uri| uri == "dart:ffi" || uri.starts_with("package:ffi/"));
    out.test_binding = binding_first(source, &scanned.tokens);
    let [no_cache, attested, portable, shardable] = markers(source, &scanned.tokens);
    if let Some(offset) = no_cache {
        out.triggers.push(Trigger {
            kind: TriggerKind::NoCache,
            label: format!("// {NO_CACHE_MARKER}"),
            line: line(offset),
        });
    }
    out.attested = attested.is_some();
    out.portable = portable.is_some();
    out.shardable = shardable.is_some();
    // One trigger per label, at its first line, in line order.
    out.triggers
        .sort_unstable_by(|a, b| a.label.cmp(&b.label).then(a.line.cmp(&b.line)));
    out.triggers
        .dedup_by(|later, first| later.label == first.label);
    out.triggers.sort_by_key(|trigger| trigger.line);
    out
}

/// The comment line a user writes to declare a suite uncacheable:
/// `// dartq: no-cache` anywhere in a file the suite reaches (its own file,
/// a helper, a shared fixture library).
pub const NO_CACHE_MARKER: &str = "dartq: no-cache";

/// The comment line vouching that a file's drifting reads never reach a
/// verdict (see [`FileFacts::attested`]).
pub const DETERMINISTIC_MARKER: &str = "dartq: deterministic";

/// The comment line vouching that a file's absolute-location reads never
/// reach a verdict except relative to the package root (see
/// [`FileFacts::portable`]).
pub const PORTABLE_MARKER: &str = "dartq: portable";

/// The comment line vouching that a suite's top-level entries may run
/// partitioned across two engines of one chunk (see
/// [`FileFacts::shardable`]). Only the suite's own file is consulted.
pub const SHARDABLE_MARKER: &str = "dartq: shardable";

/// Where the first `//` comment line carrying each of [`NO_CACHE_MARKER`],
/// [`DETERMINISTIC_MARKER`], [`PORTABLE_MARKER`] and [`SHARDABLE_MARKER`]
/// starts, as a byte offset (text after the marker is a free reason). The
/// scanner emits no tokens for comments, so comment text is what lies
/// between tokens; a marker spelled inside a string literal is a token and
/// never counts.
fn markers(source: &str, tokens: &TokenTable) -> [Option<usize>; 4] {
    let mut gap_start = 0usize;
    let mut found = [None; 4];
    let mut scan_gap = |gap_offset: usize, gap: &str| {
        let mut offset = gap_offset;
        for raw in gap.split_inclusive('\n') {
            let line_offset = offset;
            offset += raw.len();
            let line = raw.trim_end_matches(['\n', '\r']);
            if let Some(rest) = line.trim_start().strip_prefix("//") {
                let rest = rest.trim_start_matches('/').trim();
                for (slot, marker) in [
                    NO_CACHE_MARKER,
                    DETERMINISTIC_MARKER,
                    PORTABLE_MARKER,
                    SHARDABLE_MARKER,
                ]
                .iter()
                .enumerate()
                {
                    if let Some(tail) = rest.strip_prefix(marker)
                        && (tail.is_empty() || tail.starts_with(char::is_whitespace))
                    {
                        found[slot].get_or_insert(line_offset);
                    }
                }
            }
        }
    };
    for token in tokens.as_slice() {
        let range = token.byte_range();
        if range.start > gap_start {
            scan_gap(gap_start, &source[gap_start..range.start]);
        }
        gap_start = gap_start.max(range.end);
    }
    if gap_start < source.len() {
        scan_gap(gap_start, &source[gap_start..]);
    }
    found
}

/// [`FileFacts::test_binding`]: the top-level `main(…) [async] { … }`
/// reaches a binding install through declaration skeleton alone.
fn binding_first(source: &str, tokens: &TokenTable) -> bool {
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map(|token| token.kind);
    let mut index = 0;
    while index < slice.len() {
        match slice[index].kind {
            // Class, extension and initializer bodies are not top level.
            TokenKind::OpenCurly | TokenKind::OpenParen | TokenKind::OpenSquare => {
                match tokens.matching_end(index) {
                    Some(end) => index = end + 1,
                    None => return false,
                }
            }
            TokenKind::Identifier
                if tokens.lexeme(source, index) == "main"
                    && kind(index + 1) == Some(TokenKind::OpenParen) =>
            {
                let Some(params_end) = tokens.matching_end(index + 1) else {
                    return false;
                };
                let mut body = params_end + 1;
                if kind(body) == Some(TokenKind::Keyword(Keyword::Async)) {
                    body += 1;
                }
                if kind(body) != Some(TokenKind::OpenCurly) {
                    return false;
                }
                let Some(end) = tokens.matching_end(body) else {
                    return false;
                };
                return Skeleton { source, tokens }.block(body + 1, end) == Some(true);
            }
            _ => index += 1,
        }
    }
    false
}

/// The declaration skeleton of a test `main` (see [`binding_first`]).
/// `Some(true)`: a binding install was reached; `Some(false)`: the span
/// ended without one; `None`: code that is not skeleton runs first.
struct Skeleton<'a> {
    source: &'a str,
    tokens: &'a TokenTable,
}

impl Skeleton<'_> {
    fn kind(&self, index: usize) -> Option<TokenKind> {
        self.tokens.as_slice().get(index).map(|token| token.kind)
    }

    /// Statements `from..end` of a block.
    fn block(&self, mut index: usize, end: usize) -> Option<bool> {
        while index < end {
            match self.kind(index)? {
                TokenKind::Semicolon => index += 1,
                TokenKind::Identifier => {
                    let name = self.tokens.lexeme(self.source, index);
                    if TEST_BINDINGS.contains(&name)
                        && self.kind(index + 1) == Some(TokenKind::Period)
                        && self.kind(index + 2) == Some(TokenKind::Identifier)
                        && self.tokens.lexeme(self.source, index + 2) == "ensureInitialized"
                        && self.kind(index + 3) == Some(TokenKind::OpenParen)
                        && self.kind(index + 4) == Some(TokenKind::CloseParen)
                    {
                        return Some(true);
                    }
                    if self.kind(index + 1) != Some(TokenKind::OpenParen) {
                        return None;
                    }
                    let close = self.tokens.matching_end(index + 1)?;
                    match name {
                        // Its arguments are evaluated before it installs
                        // the binding.
                        "testWidgets" => {
                            self.arguments(index + 2, close, false)?;
                            return Some(true);
                        }
                        // A group body runs while the suite declares.
                        "group" => {
                            if self.arguments(index + 2, close, true)? {
                                return Some(true);
                            }
                        }
                        // Their closures run after every declaration.
                        "test" | "setUp" | "setUpAll" | "tearDown" | "tearDownAll" => {
                            self.arguments(index + 2, close, false)?;
                        }
                        _ => return None,
                    }
                    index = close + 1;
                }
                _ => return None,
            }
        }
        Some(false)
    }

    /// Call arguments `from..end`: string literals, literal named
    /// arguments, and closures — whose bodies are walked as blocks when
    /// `run_now` (a `group` body), skipped otherwise.
    fn arguments(&self, mut index: usize, end: usize, run_now: bool) -> Option<bool> {
        while index < end {
            match self.kind(index)? {
                TokenKind::String | TokenKind::Comma => index += 1,
                TokenKind::Identifier
                    if self.kind(index + 1) == Some(TokenKind::Colon)
                        && matches!(
                            self.kind(index + 2),
                            Some(
                                TokenKind::String
                                    | TokenKind::Int
                                    | TokenKind::Keyword(
                                        Keyword::True | Keyword::False | Keyword::Null
                                    )
                            )
                        )
                        && matches!(
                            self.kind(index + 3),
                            Some(TokenKind::Comma | TokenKind::CloseParen)
                        ) =>
                {
                    index += 3;
                }
                TokenKind::OpenParen => {
                    let mut body = self.tokens.matching_end(index)? + 1;
                    if self.kind(body) == Some(TokenKind::Keyword(Keyword::Async)) {
                        body += 1;
                    }
                    match self.kind(body)? {
                        TokenKind::OpenCurly => {
                            let body_end = self.tokens.matching_end(body)?;
                            if run_now && self.block(body + 1, body_end)? {
                                return Some(true);
                            }
                            index = body_end + 1;
                        }
                        // An arrow body runs later; skip to the next
                        // argument.
                        TokenKind::Function if !run_now => {
                            index = body + 1;
                            while index < end && self.kind(index) != Some(TokenKind::Comma) {
                                index = if self.kind(index)?.is_begin_group() {
                                    self.tokens.matching_end(index)? + 1
                                } else {
                                    index + 1
                                };
                            }
                        }
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        Some(false)
    }
}

/// Walk the directive header: optional script tag, annotations, `library`,
/// then `import`/`export`/`part` until the first declaration, each URI's
/// byte offset pushed to `offsets` alongside it. `None` when a directive
/// is unreadable.
fn read_directives(
    source: &str,
    tokens: &TokenTable,
    uris: &mut Vec<String>,
    offsets: &mut Vec<usize>,
) -> Option<()> {
    let slice = tokens.as_slice();
    let mut index = 0;
    while index < slice.len() {
        index = match slice[index].kind {
            TokenKind::ScriptTag => index + 1,
            TokenKind::At => skip_annotation(tokens, index),
            TokenKind::Keyword(Keyword::Library) => skip_to_semicolon(tokens, index)?,
            TokenKind::Keyword(Keyword::Import | Keyword::Export | Keyword::Part) => {
                read_directive(source, tokens, index, uris, offsets)?
            }
            _ => return Some(()),
        };
    }
    Some(())
}

/// `@name(.name)*(<args>)?((args))?` — the argument groups are skipped whole.
fn skip_annotation(tokens: &TokenTable, at: usize) -> usize {
    let slice = tokens.as_slice();
    let mut index = at + 1;
    while index < slice.len() {
        match slice[index].kind {
            TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::Period => index += 1,
            TokenKind::Lt => match tokens.matching_end(index) {
                Some(end) => index = end + 1,
                None => return index + 1,
            },
            TokenKind::OpenParen => {
                return tokens.matching_end(index).map_or(index + 1, |end| end + 1);
            }
            _ => return index,
        }
    }
    index
}

/// Index just past the `;` closing the directive at `at`, skipping nested
/// groups; `None` when the file ends first.
fn skip_to_semicolon(tokens: &TokenTable, at: usize) -> Option<usize> {
    let slice = tokens.as_slice();
    let mut index = at + 1;
    while index < slice.len() {
        match slice[index].kind {
            TokenKind::Semicolon => return Some(index + 1),
            TokenKind::Eof => return None,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                index = tokens.matching_end(index)? + 1;
            }
            _ => index += 1,
        }
    }
    None
}

/// One `import`/`export`/`part` directive. `part of` names the owning
/// library, which is never a dependency of the part. Returns the index
/// past the closing `;`.
fn read_directive(
    source: &str,
    tokens: &TokenTable,
    at: usize,
    uris: &mut Vec<String>,
    offsets: &mut Vec<usize>,
) -> Option<usize> {
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map(|token| token.kind);
    if kind(at) == Some(TokenKind::Keyword(Keyword::Part))
        && kind(at + 1) == Some(TokenKind::Keyword(Keyword::Of))
    {
        return skip_to_semicolon(tokens, at);
    }
    let mut index = at + 1;
    let mut found = false;
    while index < slice.len() {
        match slice[index].kind {
            TokenKind::Semicolon => return found.then_some(index + 1),
            TokenKind::Eof => return None,
            // A configuration test `if (dart.library.io == 'true')`: its
            // string is a condition value, not a URI.
            TokenKind::OpenParen => index = tokens.matching_end(index)? + 1,
            TokenKind::String => {
                // Adjacent literals concatenate (`'src/' 'a.dart'`).
                offsets.push(slice[index].byte_range().start);
                let mut uri = String::new();
                while kind(index) == Some(TokenKind::String) {
                    uri.push_str(unquote(tokens.lexeme(source, index))?);
                    index += 1;
                }
                uris.push(uri);
                found = true;
            }
            // Interpolation inside a URI is a compile error the cache
            // will not second-guess.
            TokenKind::StringInterpolationExpression | TokenKind::StringInterpolationIdentifier => {
                return None;
            }
            _ => index += 1,
        }
    }
    None
}

/// The literal text of one complete string token, without prefix and
/// quotes; `None` for escapes (whose decoded value the cache will not
/// guess) and for triple-quoted or unterminated forms.
fn unquote(lexeme: &str) -> Option<&str> {
    let raw = lexeme.starts_with("r'") || lexeme.starts_with("r\"");
    let quoted = if raw { &lexeme[1..] } else { lexeme };
    let quote = quoted.chars().next()?;
    if !matches!(quote, '\'' | '"') || quoted.len() < 2 || !quoted.ends_with(quote) {
        return None;
    }
    let body = &quoted[1..quoted.len() - 1];
    if body.starts_with(quote) || (!raw && body.contains('\\')) {
        return None;
    }
    Some(body)
}

/// Record the run-time inputs: ambient accessors, path literals,
/// environment reads, and the drifting reads ([`FileFacts::triggers`]);
/// `line` maps a byte offset to its 1-based line.
fn read_runtime_inputs(
    source: &str,
    tokens: &TokenTable,
    line: &impl Fn(usize) -> u32,
    out: &mut FileFacts,
) {
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map(|token| token.kind);
    let path_imports = path_imports(source, tokens);
    let mut found: Vec<Trigger> = Vec::new();
    for (index, token) in slice.iter().enumerate() {
        let mut trigger = |kind: TriggerKind, label: String| {
            found.push(Trigger {
                kind,
                label,
                line: line(token.byte_range().start),
            });
        };
        match token.kind {
            TokenKind::Identifier => {
                let lexeme = tokens.lexeme(source, index);
                let after_period = index.checked_sub(1).and_then(kind) == Some(TokenKind::Period);
                out.http_overrides |= matches!(
                    lexeme,
                    "HttpOverrides" | "overrideHttpClient" | "IntegrationTestWidgetsFlutterBinding"
                ) || (lexeme == "compute"
                    && !after_period
                    && kind(index + 1) == Some(TokenKind::OpenParen))
                    || (lexeme == "Isolate"
                        && kind(index + 1) == Some(TokenKind::Period)
                        && kind(index + 2) == Some(TokenKind::Identifier)
                        && matches!(
                            tokens.lexeme(source, index + 2),
                            "spawn" | "spawnUri" | "run"
                        ));
                out.engine_shared |= matches!(
                    lexeme,
                    "FontLoader" | "loadFontFromList" | "IsolateNameServer"
                );
                // Constructors and top-level reads of drifting sources
                // (`HttpClient()`, dart:io's `stdin` getter). A named
                // argument or a declaration of the same name is not a read.
                if !after_period {
                    let constructed = matches!(
                        kind(index + 1),
                        Some(TokenKind::OpenParen | TokenKind::Period)
                    );
                    let read = match lexeme {
                        "HttpClient" if constructed => Some(TriggerKind::Http),
                        "HttpServer" if constructed => Some(TriggerKind::Drift),
                        // Not a named argument, a declaration's name
                        // (`Object? stdin`), or an assignment target.
                        "stdin"
                            if !matches!(
                                kind(index + 1),
                                Some(TokenKind::Colon | TokenKind::Eq)
                            ) && !matches!(
                                index.checked_sub(1).and_then(kind),
                                Some(
                                    TokenKind::Identifier
                                        | TokenKind::Keyword(_)
                                        | TokenKind::Question
                                )
                            ) =>
                        {
                            Some(TriggerKind::Drift)
                        }
                        _ => None,
                    };
                    if let Some(read) = read {
                        trigger(
                            read,
                            match lexeme {
                                "stdin" => "stdin".to_owned(),
                                _ => format!("{lexeme}()"),
                            },
                        );
                    }
                }
                if after_period
                    && matches!(
                        lexeme,
                        "absolute" | "resolveSymbolicLinks" | "resolveSymbolicLinksSync"
                    )
                {
                    out.located = true;
                }
                // `io.stdin` through a prefixed `dart:io` import.
                if after_period
                    && lexeme == "stdin"
                    && index >= 2
                    && kind(index - 2) == Some(TokenKind::Identifier)
                    && kind(index + 1) == Some(TokenKind::Period)
                {
                    trigger(TriggerKind::Drift, "stdin".to_owned());
                }
                // package:path's top-level functions, unprefixed.
                if path_imports.unprefixed && !after_period {
                    match lexeme {
                        "current" => {
                            out.ambient = true;
                            out.located = true;
                            continue;
                        }
                        "absolute" | "relative" | "canonicalize"
                            if kind(index + 1) == Some(TokenKind::OpenParen) =>
                        {
                            out.located = true;
                        }
                        _ => {}
                    }
                }
                if kind(index + 1) != Some(TokenKind::Period)
                    || !matches!(
                        kind(index + 2),
                        Some(TokenKind::Identifier | TokenKind::Keyword(_))
                    )
                {
                    continue;
                }
                let member = tokens.lexeme(source, index + 2);
                if (lexeme, member) == ("Platform", "environment")
                    && !read_environment_access(source, tokens, index + 3, out)
                {
                    trigger(
                        TriggerKind::EnvOpaque,
                        "Platform.environment (unnamed key)".to_owned(),
                    );
                }
                if matches!((lexeme, member), ("DateTime", "now" | "timestamp")) {
                    trigger(TriggerKind::Clock, format!("DateTime.{member}"));
                }
                // The dart:io members that themselves perform a syscall
                // — connect/bind/lookup/list and the SecureSocket statics
                // that start a TLS handshake on a live socket. Configuration
                // (`SecureSocket.badCertificateCallback`, `setOption`) and
                // cached properties (`remoteAddress`, `port`) observe no
                // external state and do not fire.
                if matches!(
                    (lexeme, member),
                    ("Process", "run" | "runSync" | "start")
                        | ("Platform", "localHostname")
                        | ("Socket" | "RawSocket", "connect" | "startConnect")
                        | ("SecureSocket" | "RawSecureSocket", "connect" | "startConnect" | "secure" | "secureServer")
                        | (
                            "ServerSocket"
                                | "RawServerSocket"
                                | "SecureServerSocket"
                                | "RawSecureServerSocket"
                                | "RawDatagramSocket",
                            "bind"
                        )
                        | ("WebSocket", "connect" | "fromUpgradedSocket")
                        | ("InternetAddress", "lookup")
                        | ("NetworkInterface", "list")
                ) {
                    trigger(TriggerKind::Drift, format!("{lexeme}.{member}"));
                }
                // `Directory.current = …` moves every engine's directory.
                out.engine_shared |= (lexeme, member) == ("Directory", "current")
                    && kind(index + 3) == Some(TokenKind::Eq);
                match (lexeme, member) {
                    ("Process", _) | ("Directory", "current") => out.ambient = true,
                    (prefix, "current") if path_imports.prefixes.contains(&prefix) => {
                        out.ambient = true;
                    }
                    ("Platform", "packageConfig" | "resolvedExecutable")
                    | ("Uri", "base")
                    | (
                        "Isolate",
                        "packageConfig"
                        | "packageConfigSync"
                        | "resolvePackageUri"
                        | "resolvePackageUriSync",
                    ) => out.ambient = true,
                    (prefix, "absolute" | "relative" | "canonicalize")
                        if path_imports.prefixes.contains(&prefix) =>
                    {
                        out.located = true;
                    }
                    ("StackTrace", "current") => out.located = true,
                    ("Platform", "script" | "executableArguments") => {
                        out.launch = true;
                        out.ambient = true;
                    }
                    _ => {}
                }
            }
            TokenKind::String => {
                let body = segment_body(tokens.lexeme(source, index));
                // A segment right after an interpolation continues a path
                // built at run time (`'$base/lib'`): its leading `/` joins,
                // it does not make the path absolute.
                let continued = index.checked_sub(1).and_then(kind) == Some(TokenKind::CloseCurly)
                    || index >= 2
                        && index.checked_sub(2).and_then(kind)
                            == Some(TokenKind::StringInterpolationIdentifier);
                if !continued && let Some(path) = absolute_path(body) {
                    out.absolute_paths.push(path);
                }
                let body = if continued {
                    body.trim_start_matches('/')
                } else {
                    body
                };
                if let Some(path) = path_candidate(body) {
                    out.path_literals.push(path);
                }
            }
            _ => {}
        }
    }
    out.triggers.extend(found);
    out.located |= out.ambient;
    out.path_literals.sort_unstable();
    out.path_literals.dedup();
    out.absolute_paths.sort_unstable();
    out.absolute_paths.dedup();
    out.env_reads.sort_unstable();
    out.env_reads.dedup();
}

/// One `Platform.environment` access at `at` (the token index just past the
/// `environment` identifier): a literal index or `containsKey` names the
/// variable read; every other shape — iteration, `.length`, an aliased map,
/// a computed key — leaves the read unnamable and returns `false` (the
/// caller records a [`TriggerKind::EnvOpaque`] trigger).
fn read_environment_access(
    source: &str,
    tokens: &TokenTable,
    at: usize,
    out: &mut FileFacts,
) -> bool {
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map(|token| token.kind);
    match kind(at) {
        // `Platform.environment['NAME' …]`: adjacent string literals
        // concatenate (`'N' 'AME'`); a literal followed by anything but
        // `]` is a computed key.
        Some(TokenKind::OpenSquare) => {
            if let Some(name) = string_literal_run(source, tokens, at + 1) {
                if kind(at + 1 + name.1) == Some(TokenKind::CloseSquare) {
                    out.env_reads.push(name.0);
                    return true;
                }
            }
            false
        }
        // `Platform.environment.containsKey('NAME')`.
        Some(TokenKind::Period)
            if kind(at + 2) == Some(TokenKind::OpenParen)
                && tokens.lexeme(source, at + 1) == "containsKey" =>
        {
            if let Some(name) = string_literal_run(source, tokens, at + 3)
                && kind(at + 3 + name.1) == Some(TokenKind::CloseParen)
            {
                out.env_reads.push(name.0);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// The concatenation of the string-literal token run starting at `index`
/// (`'a' 'b'` → `ab`), with the run's token count. `None` when the token
/// there is not a plain literal (an escape or an interpolation makes the
/// value unnamable).
fn string_literal_run(source: &str, tokens: &TokenTable, index: usize) -> Option<(String, usize)> {
    let mut value = String::new();
    let mut count = 0;
    while tokens.as_slice().get(index + count).map(|t| t.kind) == Some(TokenKind::String) {
        value.push_str(unquote(tokens.lexeme(source, index + count))?);
        count += 1;
    }
    (count > 0).then_some((value, count))
}

/// How the file imports `package:path`: whose `current` getter (and the
/// cwd-relative `absolute`/`relative` family behind it) reads the process
/// location.
#[derive(Default)]
struct PathImports<'s> {
    prefixes: Vec<&'s str>,
    unprefixed: bool,
}

fn path_imports<'s>(source: &'s str, tokens: &TokenTable) -> PathImports<'s> {
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map(|token| token.kind);
    let mut out = PathImports::default();
    for index in 0..slice.len() {
        if kind(index) != Some(TokenKind::Keyword(Keyword::Import))
            || kind(index + 1) != Some(TokenKind::String)
            || unquote(tokens.lexeme(source, index + 1)) != Some("package:path/path.dart")
        {
            continue;
        }
        if kind(index + 2) == Some(TokenKind::Keyword(Keyword::As))
            && kind(index + 3) == Some(TokenKind::Identifier)
        {
            out.prefixes.push(tokens.lexeme(source, index + 3));
        } else {
            out.unprefixed = true;
        }
    }
    out
}

/// The text of one string token segment: the `r` prefix and whichever
/// quotes it carries stripped (a segment before an interpolation has no
/// closing quote, one after it no opening quote).
fn segment_body(lexeme: &str) -> &str {
    let lexeme = if lexeme.starts_with("r'") || lexeme.starts_with("r\"") {
        &lexeme[1..]
    } else {
        lexeme
    };
    lexeme
        .trim_start_matches(['\'', '"'])
        .trim_end_matches(['\'', '"'])
}

/// The absolute path a literal spells, if any: `/etc/hosts`, or the path
/// of `file:///tmp/x` (`file://host/x` keeps its host-relative path). A
/// bare separator (`'/'`), a comment opener (`'//'`, `'/*'`), and prose
/// with whitespace are not paths. Percent escapes are left as spelled: a
/// path the cache cannot decode with certainty keys on its spelling, which
/// names no file and so never matches a real one by accident — the suite
/// is still keyed on that absent candidate, and reruns if it appears.
fn absolute_path(text: &str) -> Option<String> {
    let path = match text.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None => text,
    };
    let rest = path.strip_prefix('/')?;
    let plausible = !rest.is_empty()
        && !rest.starts_with('/')
        && !rest.starts_with('*')
        && rest
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        && !path
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b == b'$' || b == b'\\');
    plausible.then(|| path.trim_end_matches('/').to_owned())
}

/// Path-shaped words of a data file (YAML, JSON, a manifest listing
/// fixtures): runs between whitespace, quotes, and structural punctuation
/// that look like relative paths. A suite reading a data file reads the
/// files it names too, so those words widen the suite's inputs exactly as
/// path literals in Dart code do.
#[must_use]
pub fn data_paths(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut out: Vec<String> = text
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\''
                        | '`'
                        | ','
                        | ';'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '('
                        | ')'
                        | '<'
                        | '>'
                        | '='
                )
        })
        .map(|word| word.trim_end_matches(':'))
        .filter(|word| {
            word.contains('/')
                || word.rsplit_once('.').is_some_and(|(stem, ext)| {
                    !stem.is_empty()
                        && (1..=8).contains(&ext.len())
                        && ext.bytes().all(|b| b.is_ascii_alphanumeric())
                        && ext.bytes().any(|b| b.is_ascii_alphabetic())
                })
        })
        .filter_map(path_candidate)
        .filter(|word| word != ".")
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Normalize a literal into a relative path candidate: `./` prefixes and
/// `/` ends stripped; `.` names the package root.
fn path_candidate(text: &str) -> Option<String> {
    if text.is_empty()
        || text.len() > MAX_PATH_LITERAL
        || text.starts_with('/')
        || text
            .bytes()
            .any(|b| b.is_ascii_whitespace() || matches!(b, b'\\' | b'$' | b':'))
    {
        return None;
    }
    if matches!(text, "." | "./") {
        return Some(".".to_owned());
    }
    let trimmed = text.trim_start_matches("./").trim_end_matches('/');
    (!trimmed.is_empty() && !trimmed.contains("//")).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(source: &str) -> FileFacts {
        facts(source.as_bytes())
    }

    #[test]
    fn directives_cover_every_form() {
        let facts = of(r#"#!/usr/bin/env dart
// import 'commented.dart';
/* import 'block.dart'; */
@TestOn('vm')
@Tags(<String>['a'])
library;

import 'package:test/test.dart' show test, expect;
import r'a.dart' deferred as a;
import 'stub.dart'
    if (dart.library.io) 'io.dart'
    if (dart.library.html == 'true') "web.dart";
export 'src/' 'joined.dart' hide x;
part 'part.dart';

void main() { const s = "import 'string.dart';"; }
"#);
        assert!(!facts.opaque);
        assert_eq!(
            facts.uris,
            [
                "package:test/test.dart",
                "a.dart",
                "stub.dart",
                "io.dart",
                "web.dart",
                "src/joined.dart",
                "part.dart"
            ]
        );
    }

    #[test]
    fn part_of_is_not_a_dependency() {
        let facts = of("part of 'owner.dart';\nimport 'x.dart';\nclass A {}\n");
        assert!(!facts.opaque);
        assert_eq!(facts.uris, ["x.dart"]);
        assert_eq!(of("\u{feff}import 'bom.dart';").uris, ["bom.dart"]);
    }

    #[test]
    fn unreadable_headers_are_opaque() {
        assert!(of("import 'a$b.dart';\n").opaque);
        assert!(of("import 'a${b}.dart';\n").opaque);
        assert!(of("import 'a\\u0041.dart';\n").opaque);
        assert!(of("import '''a.dart''';\n").opaque);
        assert!(of("import 'a.dart'\n").opaque);
        assert!(of("import ;\n").opaque);
        assert!(facts(&[0xff, 0xfe, b'x']).opaque);
        assert!(!of("void main() {}").opaque);
    }

    #[test]
    fn runtime_inputs() {
        let facts = of(r#"
import 'dart:io';
void main() {
  final cwd = Directory.current.path;
  File('./test/resources/x.dart/');
  print('hello world');
  print('${cwd}/lib');
  print('$cwd/bin');
  print('/etc/hosts');
}
"#);
        assert!(facts.ambient);
        // `dart:io` is a URI, `hello world` prose; the interpolation tails
        // `/lib`, `/bin` still spell package paths.
        // Absolute `/etc/hosts` is outside the package.
        assert_eq!(facts.path_literals, ["bin", "lib", "test/resources/x.dart"]);

        let path = "import 'package:path/path.dart' as p;\nvoid main() { print(p.current); }";
        assert!(of(path).ambient);
        let bare = "import 'package:path/path.dart';\nvoid main() { print(current); }";
        assert!(of(bare).ambient);
        assert!(of("void main() { Process.run('x', []); }").ambient);
        assert!(of("void main() { print(Uri.base); }").ambient);
        // `.current` of anything else is runtime state, not the cwd.
        assert!(!of("void main() { Zone.current; it.current; var current = 1; }").ambient);
        assert_eq!(of("void main() { File('.'); }").path_literals, ["."]);
    }

    /// Location observers pin a result to its checkout without widening
    /// its inputs; every ambient read observes the location too.
    #[test]
    fn drift_markers_and_launch() {
        let drift = |source: &str| of(source).nondeterministic();
        let time = |source: &str| of(source).time_read();
        assert!(time("void main() { DateTime.now(); }"));
        assert!(time("void main() { DateTime.timestamp(); }"));
        assert!(!drift("void main() { DateTime.now(); }"));
        assert!(drift("void main() { Process.run('ls', []); }"));
        assert!(drift("void main() { Socket.connect('h', 1); }"));
        assert!(drift("void main() { SecureSocket.connect('h', 1); }"));
        assert!(drift("void main() { SecureSocket.secure(s); }"));
        assert!(drift("void main() { InternetAddress.lookup('h'); }"));
        assert!(drift("void main() { WebSocket.connect('wss://h'); }"));
        assert!(drift("void main() { ServerSocket.bind('h', 1); }"));
        assert!(drift("void main() { NetworkInterface.list(); }"));
        // Configuration members of those types perform no IO: pinning a
        // certificate callback or context, tuning socket options, reading
        // cached connection fields, renegotiating an open session.
        assert!(!drift(
            "void main() { SecureSocket.badCertificateCallback = (c) => true; }"
        ));
        assert!(!drift(
            "void main() { s.setTrustedCertificates('p'); s.setAlpnProtocols(p, false); }"
        ));
        assert!(!drift(
            "void main() { s.setOption(SocketOption.tcpNoDelay, true); s.renegotiate(); }"
        ));
        assert!(!drift(
            "void main() { print(s.remoteAddress); print(s.remotePort); print(s.encoding); }"
        ));
        assert!(!drift(
            "void main() { Socket.direction; SecureSocket.peerCertificate; }"
        ));
        assert!(!drift("void main() { HttpClient(); }"));
        assert!(of("void main() { HttpClient(); }").http_read());
        assert!(drift("void main() { stdin.readLineSync(); }"));
        assert!(drift("void main() { Platform.localHostname; }"));
        assert!(drift("import 'dart:ffi';\nvoid main() {}"));
        assert!(time("import 'package:clock/clock.dart';\nvoid main() {}"));
        assert!(of("import 'package:http/http.dart' as http;\nvoid main() {}").http_read());
        assert!(drift("import 'package:grpc/grpc.dart';\nvoid main() {}"));
        // Stationary randomness, a `stdin:` named argument, a field named
        // `stdin`, and a `DateTime` constructed from a literal do not drift.
        assert!(!drift(
            "void main() { Random(1).nextInt(3); Stopwatch()..start(); }"
        ));
        assert!(!drift(
            "void f({Object? stdin}) {} void main() { f(stdin: 1); }"
        ));
        assert!(drift(
            "import 'dart:io' as io;\nvoid main() { io.stdin.readLineSync(); }"
        ));
        assert!(!drift(
            "class A { Object? stdin; } void main() { DateTime(2020); }"
        ));
        assert!(!drift(
            "import 'package:http_parser/http_parser.dart';\nvoid main() {}"
        ));

        let absolute = |source: &str| of(source).absolute_paths;
        assert_eq!(
            absolute("void main() { File('/etc/hosts'); }"),
            ["/etc/hosts"]
        );
        assert_eq!(
            absolute("void main() { Uri.parse('file:///tmp/x/'); }"),
            ["/tmp/x"]
        );
        assert!(
            absolute("void main() { s.split('/'); x('// a'); y('/* c */'); print('$a/lib'); }")
                .is_empty()
        );

        assert_eq!(
            data_paths(b"files:\n  - test/resources/a.dart\n  - 'b.yaml'\nname: x\nversion: 1.2\n"),
            ["b.yaml", "test/resources/a.dart"]
        );

        let shared = |source: &str| of(source).engine_shared;
        assert!(shared("void main() { FontLoader('x')..load(); }"));
        assert!(shared(
            "import 'dart:ui' as ui;\nvoid main() { ui.loadFontFromList(b); }"
        ));
        assert!(shared(
            "void main() { IsolateNameServer.lookupPortByName('p'); }"
        ));
        assert!(shared("import 'dart:ffi';\nvoid main() {}"));
        assert!(shared("import 'package:ffi/ffi.dart';\nvoid main() {}"));
        assert!(shared("void main() { Directory.current = '/tmp'; }"));
        // Reading the directory is a location read, not shared engine state.
        assert!(!shared("void main() { print(Directory.current.path); }"));
        assert!(!shared("void main() { Fonts.loader; }"));

        let launch = of("void main() { print(Platform.script); }");
        assert!(launch.launch && launch.ambient);
        assert!(!of("void main() {}").launch);

        let marked = of(
            "// dartq: no-cache talks to a local daemon\n// dartq: deterministic\n// dartq: portable\n// dartq: shardable\nvoid main() {}",
        );
        assert!(marked.no_cache() && marked.attested && marked.portable && marked.shardable);
        // A marker inside a string, a longer word, or a block comment does not count.
        let unmarked = of(
            "/* dartq: no-cache */\n// dartq: no-cacheable\n// dartq: shardablex\nvoid main() { print('// dartq: no-cache'); }",
        );
        assert!(
            !unmarked.no_cache() && !unmarked.attested && !unmarked.portable && !unmarked.shardable
        );
    }

    /// Location observers pin a result to its checkout without widening
    /// its inputs; every ambient read observes the location too.
    #[test]
    fn location_observers() {
        let located = |source: &str| {
            let facts = of(source);
            (facts.located, facts.ambient)
        };
        assert_eq!(
            located("void main() { File('a').absolute.path; }"),
            (true, false)
        );
        assert_eq!(
            located("void main() { Directory('a').resolveSymbolicLinksSync(); }"),
            (true, false)
        );
        assert_eq!(
            located("void main() { print(StackTrace.current); }"),
            (true, false)
        );
        let prefixed = "import 'package:path/path.dart' as p;\nvoid main() { p.absolute('a'); p.relative('b'); }";
        assert_eq!(located(prefixed), (true, false));
        let bare = "import 'package:path/path.dart';\nvoid main() { canonicalize('a'); }";
        assert_eq!(located(bare), (true, false));
        assert_eq!(located("void main() { Directory.current; }"), (true, true));
        // Unrelated members of the same names are not location reads.
        assert_eq!(
            located("void main() { relative('a'); x.canonicalize; Zone.current; }"),
            (false, false)
        );
    }

    /// `Platform.executableArguments` observes the launch exactly as
    /// `Platform.script` does.
    #[test]
    fn executable_arguments_observe_the_launch_too() {
        let facts = of("void main() { print(Platform.executableArguments); }");
        assert!(facts.launch && facts.ambient);
    }

    /// Path-shaped words of a data file: a separator or a plausible
    /// extension makes one; absolute paths, versions and prose do not.
    #[test]
    fn data_paths_shape_rules() {
        assert_eq!(
            data_paths(b"key: value\n- lib/src/a.dart\n- 'q.dart'\n- /abs/x.dart\n- no-ext\n- v1.2\n- word\n- a.b/c\n- x.C++\n"),
            ["a.b/c", "lib/src/a.dart", "q.dart"]
        );
    }

    /// The opt-out markers match one exact spelling each.
    #[test]
    fn markers_match_one_spelling() {
        assert!(
            !of("// dartq:no-cache\nvoid main() {}").no_cache(),
            "no space after the colon"
        );
        assert!(
            !of("// dartq: no-cacheable\nvoid main() {}").no_cache(),
            "a longer word"
        );
        assert!(
            of("/// dartq: no-cache why\nvoid main() {}").no_cache(),
            "extra slashes and a reason"
        );
        assert!(
            !of("// dartq:shardable\nvoid main() {}").shardable,
            "no space after the colon"
        );
        assert!(
            !of("// dartq: shardable-suite\nvoid main() {}").shardable,
            "a longer word"
        );
        assert!(
            of("/// dartq: shardable split at groups\nvoid main() {}").shardable,
            "extra slashes and a reason"
        );
    }

    /// Environment reads: literal indexes and `containsKey` name the
    /// variable; every shape the extractor cannot name — iteration,
    /// `.length`, an alias, a computed key, a spread, an escaped literal —
    /// is opaque.
    #[test]
    fn environment_reads_are_named_or_opaque() {
        let named = of(
            "void main() {\n  print(Platform.environment['HOME']);\n  print(Platform.environment['A' 'B']);\n  if (Platform.environment.containsKey(\"CI\")) {}\n}\n",
        );
        assert!(!named.env_opaque());
        assert_eq!(named.env_reads, ["AB", "CI", "HOME"]);

        for source in [
            // Iteration, map views, size.
            "void main() { for (final e in Platform.environment.entries) {} }",
            "void main() { print(Platform.environment.keys.length); }",
            // Aliased into a variable, then indexed dynamically.
            "void main() { final env = Platform.environment; print(env[x]); }",
            // Computed keys.
            "void main() { print(Platform.environment['HO' + 'ME']); }",
            "void main() { print(Platform.environment[name]); }",
            // An escape cannot be unquoted with certainty.
            "void main() { print(Platform.environment['HO\\ME']); }",
            // Passed whole.
            "void main() { print(Platform.environment); }",
        ] {
            let facts = of(source);
            assert!(facts.env_opaque(), "opaque: {source}");
            assert!(facts.env_reads.is_empty(), "no names: {source}");
        }

        // A read inside a comment or string is not a read.
        let none = of(
            "// Platform.environment['HOME']\nvoid main() { const s = 'Platform.environment[\"X\"]'; }",
        );
        assert!(!none.env_opaque() && none.env_reads.is_empty());
    }

    /// Dot-shorthand `.environment['X']` (Dart 3.10's inferred static
    /// invocation — no `Platform` receiver spelled) is not tracked:
    /// neither a named read nor an opaque one. A suite relying on it opts
    /// out with `// dartq: no-cache` (the `env_opaque` contract). A
    /// receiver other than `Platform` is not the environment map either.
    #[test]
    fn dot_shorthand_environment_access_is_not_tracked() {
        let shorthand = of("void main() { print(.environment['HOME']); }");
        assert!(
            !shorthand.env_opaque(),
            "documented: dot-shorthand is neither named nor opaque"
        );
        assert!(shorthand.env_reads.is_empty());
        let other_receiver = of("void main() { print(other.environment['HOME']); }");
        assert!(!other_receiver.env_opaque() && other_receiver.env_reads.is_empty());
    }

    /// Every trigger carries its kind, its label as spelled and its first
    /// line; one per label, in line order.
    #[test]
    fn triggers_carry_kind_label_and_line() {
        let facts = of("import 'package:http/http.dart';\n\
             // dartq: no-cache flaky daemon\n\
             void main() {\n\
               Process.run('ls', []);\n\
               DateTime.now();\n\
               DateTime.now();\n\
               print(Platform.environment[name]);\n\
             }\n");
        let seen: Vec<(TriggerKind, &str, u32)> = facts
            .triggers
            .iter()
            .map(|t| (t.kind, t.label.as_str(), t.line))
            .collect();
        assert_eq!(
            seen,
            [
                (TriggerKind::Http, "import package:http/http.dart", 1),
                (TriggerKind::NoCache, "// dartq: no-cache", 2),
                (TriggerKind::Drift, "Process.run", 4),
                (TriggerKind::Clock, "DateTime.now", 5),
                (
                    TriggerKind::EnvOpaque,
                    "Platform.environment (unnamed key)",
                    7
                ),
            ]
        );
        // Stationary reads and marker-free files carry no evidence.
        assert!(
            of("void main() { Random(1).nextInt(3); }")
                .triggers
                .is_empty()
        );
        assert!(of("void main() {}").triggers.is_empty());
    }

    /// The flutter test binding counts only when `main` installs it before
    /// running code of its own; the overrides that take its mock
    /// `HttpClient` away are recorded.
    #[test]
    fn test_binding_and_http_overrides() {
        let bound = |source: &str| of(source).test_binding;
        assert!(bound("void main() { testWidgets('x', (t) async {}); }"));
        assert!(bound(
            "void main() { TestWidgetsFlutterBinding.ensureInitialized(); foo(); }"
        ));
        assert!(bound(
            "void main() async {\n  group('g', () {\n    setUp(() => api.reset());\n    test('t', () {}, skip: true);\n    testWidgets('w', (t) async { HttpClient(); });\n  });\n}"
        ));
        // No binding at all, a declaration of the same name, code that
        // runs before the binding, a computed description.
        assert!(!bound("void main() { test('x', () {}); }"));
        assert!(!bound("void testWidgets() {}"));
        assert!(!bound(
            "void main() { final api = Api(); testWidgets('x', (t) async {}); }"
        ));
        assert!(!bound("void main() { testWidgets(name(), (t) async {}); }"));
        assert!(!bound(
            "void main() { group('g', () => testWidgets('x', (t) async {})); }"
        ));
        // A nested `main` is not the entry point.
        assert!(!bound(
            "class A { void main() { testWidgets('x', (t) async {}); } }"
        ));
        assert!(of("void main() { HttpOverrides.global = null; }").http_overrides);
        assert!(
            of("void main() { IntegrationTestWidgetsFlutterBinding.ensureInitialized(); }")
                .http_overrides
        );
        assert!(of("void main() { compute(f, 1); Isolate.run(g); }").http_overrides);
        assert!(!of("void main() { testWidgets('x', (t) async {}); }").http_overrides);
    }
}
