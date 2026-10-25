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
    /// Reads state that drifts between runs with no file, argument or
    /// environment change: the wall clock (`DateTime.now`,
    /// `DateTime.timestamp` — dates move forward, so an expiry or calendar
    /// assertion green today can be red tomorrow), the outside world (the
    /// network: `Socket`, `ServerSocket`, `RawSocket`, `SecureSocket`,
    /// `RawDatagramSocket`, `WebSocket`, `HttpClient`, `HttpServer`,
    /// `InternetAddress`, `NetworkInterface`; child processes:
    /// `Process.run`/`runSync`/`start`; standard input), the host's
    /// identity (`Platform.localHostname`), native code (`dart:ffi`), or
    /// imports a package whose purpose is one of those
    /// ([`DRIFT_PACKAGES`]). A suite whose closure reads one is never
    /// cached.
    ///
    /// Stationary randomness (`Random`, `Stopwatch` durations, `pid`) is not
    /// listed: a replayed outcome is then one sample of the same
    /// distribution a fresh run draws from, so replay cannot observe
    /// anything a rerun could not.
    pub nondeterministic: bool,
    /// Carries the `// dartq: deterministic` attestation: the author vouches
    /// that this file's drifting reads ([`Self::nondeterministic`]) never
    /// reach a test verdict (a log timestamp, an analyzer modification
    /// stamp). Only this file's own reads are vouched for.
    pub attested: bool,
    /// Carries the `// dartq: no-cache` opt-out marker: the suite reaching
    /// this file always runs.
    pub no_cache: bool,
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
}

/// Longest literal still considered a candidate path.
const MAX_PATH_LITERAL: usize = 255;

/// Packages whose API is a drifting source: `clock` (the wall clock
/// behind an injectable seam), HTTP/WebSocket/RPC clients and servers, and
/// process spawning. Importing one from package code marks the file
/// [`FileFacts::nondeterministic`]: the drift happens inside the
/// dependency, where the cache does not look.
pub const DRIFT_PACKAGES: &[&str] = &[
    "clock", "dio", "grpc", "http", "http2", "io", "process", "process_run", "shelf",
    "web_socket", "web_socket_channel",
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
    let mut out = FileFacts::default();
    out.opaque = read_directives(source, &scanned.tokens, &mut out.uris).is_none();
    read_runtime_inputs(source, &scanned.tokens, &mut out);
    out.nondeterministic |= out.uris.iter().any(|uri| {
        uri == "dart:ffi"
            || uri
                .strip_prefix("package:")
                .and_then(|rest| rest.split_once('/'))
                .is_some_and(|(package, _)| DRIFT_PACKAGES.contains(&package))
    });
    out.engine_shared |= out.uris.iter().any(|uri| uri == "dart:ffi" || uri.starts_with("package:ffi/"));
    let [no_cache, attested, portable] = markers(source, &scanned.tokens);
    out.no_cache = no_cache;
    out.attested = attested;
    out.portable = portable;
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

/// Which of [`NO_CACHE_MARKER`], [`DETERMINISTIC_MARKER`] and
/// [`PORTABLE_MARKER`] a `//` comment line of the source carries (text
/// after the marker is a free reason). The scanner emits no tokens for
/// comments, so comment text is what lies between tokens; a marker spelled
/// inside a string literal is a token and never counts.
fn markers(source: &str, tokens: &TokenTable) -> [bool; 3] {
    let mut gap_start = 0usize;
    let mut found = [false; 3];
    let mut scan_gap = |gap: &str| {
        for line in gap.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("//") {
                let rest = rest.trim_start_matches('/').trim();
                for (slot, marker) in
                    [NO_CACHE_MARKER, DETERMINISTIC_MARKER, PORTABLE_MARKER].iter().enumerate()
                {
                    if let Some(tail) = rest.strip_prefix(marker)
                        && (tail.is_empty() || tail.starts_with(char::is_whitespace))
                    {
                        found[slot] = true;
                    }
                }
            }
        }
    };
    for token in tokens.as_slice() {
        let range = token.byte_range();
        if range.start > gap_start {
            scan_gap(&source[gap_start..range.start]);
        }
        gap_start = gap_start.max(range.end);
    }
    if gap_start < source.len() {
        scan_gap(&source[gap_start..]);
    }
    found
}

/// Walk the directive header: optional script tag, annotations, `library`,
/// then `import`/`export`/`part` until the first declaration. `None` when
/// a directive is unreadable.
fn read_directives(source: &str, tokens: &TokenTable, uris: &mut Vec<String>) -> Option<()> {
    let slice = tokens.as_slice();
    let mut index = 0;
    while index < slice.len() {
        index = match slice[index].kind {
            TokenKind::ScriptTag => index + 1,
            TokenKind::At => skip_annotation(tokens, index),
            TokenKind::Keyword(Keyword::Library) => skip_to_semicolon(tokens, index)?,
            TokenKind::Keyword(Keyword::Import | Keyword::Export | Keyword::Part) => {
                read_directive(source, tokens, index, uris)?
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

/// Record the run-time inputs: ambient accessors, path literals, and
/// environment reads.
fn read_runtime_inputs(source: &str, tokens: &TokenTable, out: &mut FileFacts) {
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map(|token| token.kind);
    let path_imports = path_imports(source, tokens);
    for (index, token) in slice.iter().enumerate() {
        match token.kind {
            TokenKind::Identifier => {
                let lexeme = tokens.lexeme(source, index);
                let after_period = index.checked_sub(1).and_then(kind) == Some(TokenKind::Period);
                out.engine_shared |= matches!(lexeme, "FontLoader" | "loadFontFromList" | "IsolateNameServer");
                // Constructors and top-level reads of drifting sources
                // (`HttpClient()`, dart:io's `stdin` getter). A named
                // argument or a declaration of the same name is not a read.
                if !after_period {
                    out.nondeterministic |= match lexeme {
                        "HttpClient" | "HttpServer" => matches!(
                            kind(index + 1),
                            Some(TokenKind::OpenParen | TokenKind::Period)
                        ),
                        // Not a named argument, a declaration's name
                        // (`Object? stdin`), or an assignment target.
                        "stdin" => !matches!(kind(index + 1), Some(TokenKind::Colon | TokenKind::Eq))
                            && !matches!(
                                index.checked_sub(1).and_then(kind),
                                Some(TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::Question)
                            ),
                        _ => false,
                    };
                }
                if after_period
                    && matches!(lexeme, "absolute" | "resolveSymbolicLinks" | "resolveSymbolicLinksSync")
                {
                    out.located = true;
                }
                // `io.stdin` through a prefixed `dart:io` import.
                out.nondeterministic |= after_period
                    && lexeme == "stdin"
                    && index >= 2
                    && kind(index - 2) == Some(TokenKind::Identifier)
                    && kind(index + 1) == Some(TokenKind::Period);
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
                    || !matches!(kind(index + 2), Some(TokenKind::Identifier | TokenKind::Keyword(_)))
                {
                    continue;
                }
                let member = tokens.lexeme(source, index + 2);
                out.nondeterministic |= matches!(
                    (lexeme, member),
                    ("DateTime", "now" | "timestamp")
                        | ("Process", "run" | "runSync" | "start")
                        | ("Platform", "localHostname")
                        | (
                            "Socket" | "ServerSocket" | "RawSocket" | "RawServerSocket"
                                | "SecureSocket" | "SecureServerSocket" | "RawDatagramSocket"
                                | "WebSocket" | "InternetAddress" | "NetworkInterface",
                            _
                        )
                );
                // `Directory.current = …` moves every engine's directory.
                out.engine_shared |=
                    (lexeme, member) == ("Directory", "current") && kind(index + 3) == Some(TokenKind::Eq);
                match (lexeme, member) {
                    ("Process", _) | ("Directory", "current") => out.ambient = true,
                    (prefix, "current") if path_imports.prefixes.contains(&prefix) => {
                        out.ambient = true;
                    }
                    ("Platform", "packageConfig" | "resolvedExecutable")
                    | ("Uri", "base")
                    | (
                        "Isolate",
                        "packageConfig" | "packageConfigSync" | "resolvePackageUri"
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
                let body = if continued { body.trim_start_matches('/') } else { body };
                if let Some(path) = path_candidate(body) {
                    out.path_literals.push(path);
                }
            }
            _ => {}
        }
    }
    out.located |= out.ambient;
    out.path_literals.sort_unstable();
    out.path_literals.dedup();
    out.absolute_paths.sort_unstable();
    out.absolute_paths.dedup();
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
    lexeme.trim_start_matches(['\'', '"']).trim_end_matches(['\'', '"'])
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
        && rest.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        && !path.bytes().any(|b| b.is_ascii_whitespace() || b == b'$' || b == b'\\');
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
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ',' | ';' | '[' | ']' | '{' | '}' | '(' | ')' | '<' | '>' | '='))
        .map(|word| word.trim_end_matches(':'))
        .filter(|word| word.contains('/') || word.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && (1..=8).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric()) && ext.bytes().any(|b| b.is_ascii_alphabetic())))
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
        || text.bytes().any(|b| b.is_ascii_whitespace() || matches!(b, b'\\' | b'$' | b':'))
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
        let drift = |source: &str| of(source).nondeterministic;
        assert!(drift("void main() { DateTime.now(); }"));
        assert!(drift("void main() { DateTime.timestamp(); }"));
        assert!(drift("void main() { Process.run('ls', []); }"));
        assert!(drift("void main() { Socket.connect('h', 1); }"));
        assert!(drift("void main() { HttpClient(); }"));
        assert!(drift("void main() { stdin.readLineSync(); }"));
        assert!(drift("void main() { Platform.localHostname; }"));
        assert!(drift("import 'dart:ffi';\nvoid main() {}"));
        assert!(drift("import 'package:clock/clock.dart';\nvoid main() {}"));
        assert!(drift("import 'package:http/http.dart' as http;\nvoid main() {}"));
        // Stationary randomness, a `stdin:` named argument, a field named
        // `stdin`, and a `DateTime` constructed from a literal do not drift.
        assert!(!drift("void main() { Random(1).nextInt(3); Stopwatch()..start(); }"));
        assert!(!drift("void f({Object? stdin}) {} void main() { f(stdin: 1); }"));
        assert!(drift("import 'dart:io' as io;\nvoid main() { io.stdin.readLineSync(); }"));
        assert!(!drift("class A { Object? stdin; } void main() { DateTime(2020); }"));
        assert!(!drift("import 'package:http_parser/http_parser.dart';\nvoid main() {}"));

        let absolute = |source: &str| of(source).absolute_paths;
        assert_eq!(absolute("void main() { File('/etc/hosts'); }"), ["/etc/hosts"]);
        assert_eq!(absolute("void main() { Uri.parse('file:///tmp/x/'); }"), ["/tmp/x"]);
        assert!(absolute("void main() { s.split('/'); x('// a'); y('/* c */'); print('$a/lib'); }").is_empty());

        assert_eq!(
            data_paths(b"files:\n  - test/resources/a.dart\n  - 'b.yaml'\nname: x\nversion: 1.2\n"),
            ["b.yaml", "test/resources/a.dart"]
        );

        let shared = |source: &str| of(source).engine_shared;
        assert!(shared("void main() { FontLoader('x')..load(); }"));
        assert!(shared("import 'dart:ui' as ui;\nvoid main() { ui.loadFontFromList(b); }"));
        assert!(shared("void main() { IsolateNameServer.lookupPortByName('p'); }"));
        assert!(shared("import 'dart:ffi';\nvoid main() {}"));
        assert!(shared("import 'package:ffi/ffi.dart';\nvoid main() {}"));
        assert!(shared("void main() { Directory.current = '/tmp'; }"));
        // Reading the directory is a location read, not shared engine state.
        assert!(!shared("void main() { print(Directory.current.path); }"));
        assert!(!shared("void main() { Fonts.loader; }"));

        let launch = of("void main() { print(Platform.script); }");
        assert!(launch.launch && launch.ambient);
        assert!(!of("void main() {}").launch);

        let marked = of("// dartq: no-cache talks to a local daemon\n// dartq: deterministic\n// dartq: portable\nvoid main() {}");
        assert!(marked.no_cache && marked.attested && marked.portable);
        // A marker inside a string, a longer word, or a block comment does not count.
        let unmarked = of("/* dartq: no-cache */\n// dartq: no-cacheable\nvoid main() { print('// dartq: no-cache'); }");
        assert!(!unmarked.no_cache && !unmarked.attested && !unmarked.portable);
    }

    /// Location observers pin a result to its checkout without widening
    /// its inputs; every ambient read observes the location too.
    #[test]
    fn location_observers() {
        let located = |source: &str| {
            let facts = of(source);
            (facts.located, facts.ambient)
        };
        assert_eq!(located("void main() { File('a').absolute.path; }"), (true, false));
        assert_eq!(located("void main() { Directory('a').resolveSymbolicLinksSync(); }"), (true, false));
        assert_eq!(located("void main() { print(StackTrace.current); }"), (true, false));
        let prefixed = "import 'package:path/path.dart' as p;\nvoid main() { p.absolute('a'); p.relative('b'); }";
        assert_eq!(located(prefixed), (true, false));
        let bare = "import 'package:path/path.dart';\nvoid main() { canonicalize('a'); }";
        assert_eq!(located(bare), (true, false));
        assert_eq!(located("void main() { Directory.current; }"), (true, true));
        // Unrelated members of the same names are not location reads.
        assert_eq!(located("void main() { relative('a'); x.canonicalize; Zone.current; }"), (false, false));

    }
}
