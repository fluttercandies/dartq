//! Post-build scan of a flutter build directory into an ingest draft.
//!
//! flutter_tools 3.47.1 keeps one directory per build configuration under
//! `.dart_tool/flutter_build/`, named by 32 lowercase hex digits (see
//! [`crate::args_model::flutter_build_dir_name`]). Next to the payloads
//! (`app.dill`, AOT snapshots, `App.framework/App`, …) it holds bookkeeping
//! files that record absolute paths: `<target>.stamp` (JSON `inputs` and
//! `outputs` lists), `.filecache` (JSON `{"version": 2, "files": [{"path",
//! "hash"}]}`), `outputs.json` and the hook and native-asset `*.json`
//! results, and Make-style `*.d` depfiles. Every build rewrites
//! `.filecache` and `outputs.json` in place, so the newest contained file —
//! not the directory mtime, which only moves when entries are added or
//! removed — tells which configuration a build just used.
//!
//! Every absolute path in those bytes is classified against the ingest
//! tree's [`PathAnchors`] while the file streams through the hash: under
//! the worktree root (rewritten on a cross-tree replay), under a cache
//! root (verbatim), or FOREIGN — recorded per file as
//! [`crate::types::ManifestFile::foreign_paths`], and a dir that holds a
//! foreign file is refused by cross-tree replays (see
//! [`crate::orchestrator`]). A `flutter test` kernel dill is classified
//! the same way through its embedded `file://` URIs: a foreign URI
//! downgrades the dill to a verbatim clone ([`RewriteKind::None`]),
//! which costs a frontend_server recompile, never a wrong artifact.
//!
//! Both faces share one re-ingest fast path: a file whose recorded
//! identity matches — placed by this invocation's own materialization,
//! or stamped by the prior entry under the ingest key — is re-referenced
//! without reading a byte (its recorded class carried verbatim), so a
//! warm run stores only what flutter actually changed (see
//! [`reused_identity`]).

use std::ffi::OsStr;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use memchr::memmem;
use rayon::prelude::*;

use crate::error::{BcacheError, Result};
use crate::fsutil::{file_blake3, stamp_of};
use crate::types::{
    BlobId, DirRecord, FileStamp, ManifestFile, PathAnchors, PlacedFile, RewriteKind,
};

/// Files up to this size are read whole, one buffer serving both the hash
/// and the root search; larger bookkeeping files stream through windows of
/// this size.
const WHOLE_READ_LIMIT: usize = 1024 * 1024;

/// Longest absolute-path token the foreign classifier examines. A longer
/// `/`-headed run is not a path any filesystem serves; refusing it
/// conservatively can only cost a replay, never correctness.
const PATH_TOKEN_CAP: usize = 4096;

/// Longest URI head the dill classifier examines: anchors are absolute
/// roots (far shorter), so the prefix classification always completes
/// within this many bytes after `file://`.
const URI_HEAD_CAP: usize = 1024;

/// A token boundary for absolute paths as flutter writes them: JSON
/// string quotes, depfile spaces, and every control byte.
fn is_token_delimiter(byte: u8) -> bool {
    byte <= 0x20 || byte == b'"' || byte == b'\''
}

/// The escaped spellings a bookkeeping file may record a path in, on
/// top of the plain one: depfiles escape spaces (`\ `), JSON escapes
/// backslashes (`\\`), and a backslash-separator platform writing a
/// depfile does both. The three forms come in a fixed order so callers
/// can pair them root-by-root; a form that escapes nothing equals the
/// plain bytes.
pub(crate) fn escaped_forms(bytes: &[u8]) -> [Vec<u8>; 3] {
    let Some(text) = std::str::from_utf8(bytes).ok() else {
        return std::array::from_fn(|_| bytes.to_vec());
    };
    let json = text.replace('\\', "\\\\");
    let depfile = text.replace(' ', "\\ ");
    let both = json.replace(' ', "\\ ");
    [json.into_bytes(), depfile.into_bytes(), both.into_bytes()]
}

/// Named counts of the files a scan left OUT of the manifest — the
/// honesty rule: every skip is a counted, surfaced reason, never a
/// silent hole in an entry.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ScanSkips {
    /// A bookkeeping file over the rewrite limit: recorded, it would
    /// fail every materialization (the hit-time rewrite refuses files
    /// past the limit), so the entry never carries it — flutter
    /// rewrites the file itself on the next run.
    pub oversize_rewrite: u64,
    /// A symlink or other non-regular file: a manifest records regular
    /// files only.
    pub non_regular: u64,
    /// A file whose name is not UTF-8: manifest paths are UTF-8.
    pub non_utf8_names: u64,
}

impl ScanSkips {
    pub fn total(&self) -> u64 {
        self.oversize_rewrite + self.non_regular + self.non_utf8_names
    }

    pub(crate) fn add(&mut self, other: &ScanSkips) {
        self.oversize_rewrite += other.oversize_rewrite;
        self.non_regular += other.non_regular;
        self.non_utf8_names += other.non_utf8_names;
    }
}

impl std::fmt::Display for ScanSkips {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} oversized bookkeeping, {} non-regular, {} non-UTF-8 names",
            self.oversize_rewrite, self.non_regular, self.non_utf8_names
        )
    }
}

/// Streaming classifier of the absolute paths inside a bookkeeping
/// file. A token delimited per [`is_token_delimiter`] that starts with
/// `/` (but not `//`, a URL artifact) is classified against the
/// anchors; one that is under neither the worktree root nor a cache
/// root marks the file foreign. Depfile space-escaping and JSON
/// framing can only TRUNCATE a token after its leading components,
/// and classification is a path-component prefix test, so truncation
/// cannot flip a class.
struct ForeignPathScan<'a> {
    anchors: &'a PathAnchors,
    token: Vec<u8>,
    foreign: bool,
}

impl<'a> ForeignPathScan<'a> {
    fn new(anchors: &'a PathAnchors) -> Self {
        Self {
            anchors,
            token: Vec::with_capacity(128),
            foreign: false,
        }
    }

    fn feed(&mut self, window: &[u8]) {
        let mut escaped = false;
        for &byte in window {
            if escaped {
                // The byte after a backslash continues the token: the
                // depfile `\ ` and JSON `\\` spellings of a path are one
                // token, and its class is decided at the delimiter that
                // follows the whole thing.
                if self.token.len() < PATH_TOKEN_CAP {
                    self.token.push(byte);
                }
                escaped = false;
            } else if byte == b'\\' {
                if self.token.len() < PATH_TOKEN_CAP {
                    self.token.push(byte);
                }
                escaped = true;
            } else if is_token_delimiter(byte) {
                self.end_token();
            } else if self.token.len() < PATH_TOKEN_CAP {
                self.token.push(byte);
            }
            // Past the cap only the token's head is kept; `end_token`
            // treats a full-buffer head as unclassifiable.
        }
    }

    fn finish(&mut self) -> bool {
        self.end_token();
        self.foreign
    }

    fn end_token(&mut self) {
        // A lone `/` or a `//`-headed run is tree-independent framing
        // (a URL artifact), never foreign; classification only applies
        // to a real absolute-path spelling.
        if self.token.first() == Some(&b'/') && self.token.len() >= 2 && self.token[1] != b'/' {
            // At the cap the token was truncated mid-path — the tail is
            // unknown, so refuse conservatively (costs a replay, never
            // correctness).
            let truncated = self.token.len() >= PATH_TOKEN_CAP;
            // Escaped spellings are undone for the comparison: the
            // anchors hold real paths, the token may hold their depfile
            // or JSON spelling.
            let carries = if self.token.contains(&b'\\') {
                self.anchors.carries_absolute(&unescape(&self.token))
            } else {
                self.anchors.carries_absolute(&self.token)
            };
            if truncated || !carries {
                self.foreign = true;
            }
        }
        self.token.clear();
    }
}

/// Undoes the backslash escapes of depfile and JSON path spellings:
/// every `\X` becomes `X`. A trailing lone backslash (a token cut at
/// the cap) drops off — such tokens are refused as truncated anyway.
fn unescape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut escaped = false;
    for &byte in bytes {
        if escaped {
            out.push(byte);
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else {
            out.push(byte);
        }
    }
    out
}

/// Streaming classifier of the `file://` URIs a kernel dill embeds.
/// After each scheme match the URI's head (up to [`URI_HEAD_CAP`]
/// bytes or its first delimiter) is prefix-classified against the
/// anchors: under the worktree root or a cache root the URI replays
/// (rewritten by [`crate::dill_rewrite`] on the worktree leg); any
/// other absolute URI marks the dill foreign. Binary framing around
/// the string tables cannot mask a scheme match, and a head that runs
/// past the cap without a delimiter still shows any anchor prefix —
/// or none, which is the foreign answer.
struct ForeignUriScan<'a> {
    anchors: &'a PathAnchors,
    /// Unprocessed bytes: at most `file://`.len() - 1 carry in search
    /// mode, the collected head in URI mode.
    buf: Vec<u8>,
    in_uri: bool,
    foreign: bool,
}

const URI_SCHEME: &[u8] = b"file://";

impl<'a> ForeignUriScan<'a> {
    fn new(anchors: &'a PathAnchors) -> Self {
        Self {
            anchors,
            buf: Vec::with_capacity(URI_HEAD_CAP + URI_SCHEME.len()),
            in_uri: false,
            foreign: false,
        }
    }

    fn feed(&mut self, window: &[u8]) {
        self.buf.extend_from_slice(window);
        loop {
            if self.in_uri {
                match self.buf.iter().position(|&byte| is_token_delimiter(byte)) {
                    // Delimiter inside the head window: the head ends.
                    Some(at) if at < URI_HEAD_CAP => {
                        let head: Vec<u8> = self.buf.drain(..at).collect();
                        self.buf.remove(0);
                        self.classify(&head);
                        self.in_uri = false;
                    }
                    // No delimiter yet and the head is still short:
                    // wait for more bytes.
                    None if self.buf.len() < URI_HEAD_CAP => return,
                    // The cap bounds the head with or without a
                    // delimiter; the prefix classification is complete
                    // either way.
                    _ => {
                        let head: Vec<u8> = self.buf.drain(..URI_HEAD_CAP).collect();
                        self.classify(&head);
                        self.in_uri = false;
                    }
                }
                continue;
            }
            match memmem::find(&self.buf, URI_SCHEME) {
                Some(at) => {
                    self.buf.drain(..at + URI_SCHEME.len());
                    self.in_uri = true;
                }
                None => {
                    let keep = URI_SCHEME.len() - 1;
                    if self.buf.len() > keep {
                        let cut = self.buf.len() - keep;
                        self.buf.drain(..cut);
                    }
                    return;
                }
            }
        }
    }

    fn finish(&mut self) -> bool {
        if self.in_uri {
            let head = std::mem::take(&mut self.buf);
            self.classify(&head);
            self.in_uri = false;
        }
        self.foreign
    }

    fn classify(&mut self, head: &[u8]) {
        if head.first() == Some(&b'/')
            && head.get(1).is_some_and(|&byte| byte != b'/')
            && !self.anchors.carries_absolute(head)
        {
            self.foreign = true;
        }
    }
}

/// What a scanned file's bytes are examined for while they hash.
enum ByteScan<'a> {
    /// A bookkeeping file: the rewrite-classification needles (the
    /// worktree root; the own-dir `flutter_build/<name>` segment) and
    /// the absolute-path token scan.
    Bookkeeping {
        needles: Vec<memmem::Finder<'a>>,
        foreign: ForeignPathScan<'a>,
    },
    /// A kernel dill: the `file://` URI scan, plus — on the payload
    /// face — the worktree-root URI needle whose occurrence marks the
    /// dill replayable (the test face classifies every non-foreign dill
    /// as rewriteable and passes `None`).
    DillUris {
        foreign: ForeignUriScan<'a>,
        root_uri: Option<memmem::Finder<'a>>,
    },
}

impl ByteScan<'_> {
    /// Feeds one window to the foreign scanner; every byte of the file
    /// passes through exactly once.
    fn feed_foreign(&mut self, window: &[u8]) {
        match self {
            ByteScan::Bookkeeping { foreign, .. } => foreign.feed(window),
            ByteScan::DillUris { foreign, .. } => foreign.feed(window),
        }
    }

    /// Whether any rewrite needle occurs in `bytes` (search mode: bytes
    /// may include the previous window's carry).
    fn any_needle_in(&self, bytes: &[u8]) -> bool {
        match self {
            ByteScan::Bookkeeping { needles, .. } => {
                needles.iter().any(|needle| needle.find(bytes).is_some())
            }
            ByteScan::DillUris { root_uri, .. } => root_uri
                .as_ref()
                .is_some_and(|finder| finder.find(bytes).is_some()),
        }
    }

    /// The carry a streaming needle search must retain between windows.
    fn max_needle_len(&self) -> usize {
        match self {
            ByteScan::Bookkeeping { needles, .. } => needles
                .iter()
                .map(|needle| needle.needle().len())
                .max()
                .unwrap_or(1),
            ByteScan::DillUris { root_uri, .. } => {
                root_uri.as_ref().map_or(1, |finder| finder.needle().len())
            }
        }
    }

    /// Finishes the foreign classification.
    fn foreign(&mut self) -> bool {
        match self {
            ByteScan::Bookkeeping { foreign, .. } => foreign.finish(),
            ByteScan::DillUris { foreign, .. } => foreign.finish(),
        }
    }
}

/// Every hash dir flutter just touched, by name.
///
/// Among the entries of `flutter_build_root` that are directories named by
/// 32 lowercase hex digits (anything else, such as
/// `dart_plugin_registrant.dart`, is ignored), a directory counts as touched
/// at the later of its own mtime and the newest mtime of the regular files
/// below it. Returns the `(name, touch time)` of every dir touched at or
/// after `since`, sorted by name for determinism. Empty when the root is
/// missing or nothing was touched since.
///
/// `since` must be taken before flutter starts; on file systems that keep
/// whole-second mtimes, round it down to the second.
///
/// # Errors
///
/// [`BcacheError::Io`] when the root or a hash dir cannot be read.
pub fn find_touched_hash_dirs(
    flutter_build_root: &Path,
    since: SystemTime,
) -> Result<Vec<(String, SystemTime)>> {
    let entries = match fs::read_dir(flutter_build_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut touched: Vec<(String, SystemTime)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !is_config_hash(&name) {
            continue;
        }
        let dir_mtime = entry.metadata()?.modified()?;
        let newest_file = newest_file_mtime(&entry.path())?;
        let at = newest_file.map_or(dir_mtime, |file| file.max(dir_mtime));
        if at >= since {
            touched.push((name, at));
        }
    }
    touched.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(touched)
}

/// Whether any regular file below `dir`, or the dir itself, was modified
/// at or after `since` — the touched test of the fixed-path faces (the
/// kernel-cache scan below; `since` follows the whole-second flooring of
/// the hash-dir scan). A missing `dir` is untouched, not an error.
///
/// # Errors
///
/// [`BcacheError::Io`] when the dir or a file below it cannot be read.
pub fn dir_touched_since(dir: &Path, since: SystemTime) -> Result<bool> {
    let metadata = match fs::metadata(dir) {
        Ok(metadata) if metadata.is_dir() => metadata,
        Ok(_) => return Ok(false),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    let dir_mtime = metadata.modified()?;
    let newest_file = newest_file_mtime(dir)?;
    Ok(newest_file.map_or(dir_mtime, |file| file.max(dir_mtime)) >= since)
}

/// One scanned ingest dir: every file, manifest-shaped and stamped,
/// and the re-hashed subset — the ingest sources. A file whose identity
/// a placed or prior record proves (see [`reused_identity`]) is no
/// source: its bytes are never read. `skipped` names and counts the
/// files the scan left out (see [`ScanSkips`]).
#[derive(Clone, Debug)]
pub struct DirScan {
    pub files: Vec<ManifestFile>,
    pub sources: Vec<(PathBuf, BlobId)>,
    pub skipped: ScanSkips,
}

/// The known content one scanned file is proven to hold without reading
/// it, as `(blob, rewrite, foreign)` to reference verbatim: the blob
/// identity is established when — and only when — the file's
/// [`FileStamp`] equals a recorded stamp AND the store still holds the
/// recorded blob. The path classification rides along from the record
/// it was proven by: the worktree-root rewrite preserves it (worktree
/// paths move between trees, foreign paths do not move at all).
///
/// Two records can vouch, in this precedence:
///
/// 1. **Placed** (this invocation's own materialization): a matching
///    stamp means the file is the one the shim placed after the prior
///    run; when the placement was verbatim (the placed bytes are the
///    blob's bytes) the blob is referenced directly. A mismatching
///    stamp means the run rewrote the file — it is hashed, even when a
///    prior-entry stamp from an older ingest also happens to match.
///    That suppression is what keeps the rule honest: only the
///    placement knows the bytes that are on disk NOW.
/// 2. **Prior** (the entry stored under the ingest key): a matching
///    stamp proves no write touched the inode since the prior ingest
///    hashed it — the content is the bytes the recorded blob already
///    holds (the identity semantics proven for the test face: every
///    write moves the ns mtime, a replaced file gets a new inode).
///
/// Anything else — no record for the path, a stamp mismatch, a legacy
/// record without a stamp, a blob gc has removed, a placed-but-rewritten
/// file — yields `None` and the file is hashed, so the fast path can
/// never keep a stale blob reference for changed content. The ms-scale
/// window where gc removes the blob after `blob_exists` agreed is the
/// store's documented gc race: the ingest's upfront check rejects the
/// draft, the run loses one ingest, and the next run re-hashes.
fn reused_identity(
    file: &ScannedFile,
    prior: Option<&DirRecord>,
    placed: Option<&[PlacedFile]>,
    blob_exists: &(dyn Fn(&BlobId) -> bool + Sync),
) -> Option<(BlobId, RewriteKind, bool)> {
    if let Some(placed) =
        placed.and_then(|files| files.iter().find(|f| f.path == Path::new(&file.relative)))
    {
        return (placed.verbatim && placed.stamp == file.stamp && blob_exists(&placed.blob))
            .then_some((placed.blob, placed.rewrite, placed.foreign_paths));
    }
    prior
        .and_then(|dir| {
            dir.files.iter().find(|recorded| {
                recorded.path == Path::new(&file.relative)
                    && recorded.stamp == Some(file.stamp)
                    && blob_exists(&recorded.blob)
            })
        })
        .map(|recorded| (recorded.blob, recorded.rewrite, recorded.foreign_paths))
}

/// Scans the `flutter test` kernel-cache dir for ingest: every
/// `*.cache.dill*` file below `cache_dir` (flutter_tools' TestCompiler
/// writes `<md5>.cache.dill[.track.dill]` under it; the file name encodes
/// the config, so it doubles as the replay address), each stamped with
/// its [`FileStamp`]. A dill whose embedded `file://` URIs all sit under
/// the ingest anchors ingests as [`RewriteKind::DillUris`] (a same-tree
/// replay stays a byte-identical clone, a cross-tree replay rewrites the
/// URI prefixes, a refused rewrite degrades to the clone); one carrying
/// a foreign URI ingests as [`RewriteKind::None`] — placed verbatim
/// everywhere, costing a frontend_server recompile on a cross-tree hit,
/// never a wrong library.
///
/// `prior` is the test-cache [`DirRecord`] of the entry currently stored
/// under the ingest key; `placed` the files this invocation's
/// materialization placed into the dir (a same-tree hit places the dill
/// verbatim, so a run that left it alone re-references it without
/// reading tens of MB). Both feed [`reused_identity`].
///
/// `None` when the dir is missing, untouched since `since` (the run
/// wrote nothing — nothing to ingest), or holds no kernel-cache file.
/// Files a manifest cannot represent (a symlink, a non-UTF-8 name) are
/// skipped with a named count instead of failing the scan.
///
/// # Errors
///
/// [`BcacheError::Io`] when reading or hashing fails.
pub fn scan_test_cache(
    cache_dir: &Path,
    since: SystemTime,
    anchors: &PathAnchors,
    prior: Option<&DirRecord>,
    placed: Option<&[PlacedFile]>,
    blob_exists: &(dyn Fn(&BlobId) -> bool + Sync),
) -> Result<Option<DirScan>> {
    if !dir_touched_since(cache_dir, since)? {
        return Ok(None);
    }
    let mut scanned = Vec::new();
    let mut skipped = ScanSkips::default();
    collect_files(cache_dir, &mut scanned, &mut skipped)?;
    scanned.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
    let mut files = Vec::new();
    let mut sources = Vec::new();
    for file in scanned {
        if !is_kernel_cache(Path::new(&file.relative)) {
            continue;
        }
        let (blob, rewrite) = match reused_identity(&file, prior, placed, blob_exists) {
            Some((blob, rewrite, _)) => (blob, rewrite),
            None => {
                let (blob, _, foreign) = digest(
                    &file.absolute,
                    file.size,
                    Some(&mut ByteScan::DillUris {
                        foreign: ForeignUriScan::new(anchors),
                        root_uri: None,
                    }),
                )?;
                sources.push((file.absolute.clone(), blob));
                // The dill face encodes its classification in the kind
                // alone: a foreign dill is a verbatim clone everywhere,
                // never a dir-refusing foreign_paths record.
                let rewrite = if foreign {
                    RewriteKind::None
                } else {
                    RewriteKind::DillUris
                };
                (blob, rewrite)
            }
        };
        files.push(ManifestFile {
            path: PathBuf::from(&file.relative),
            blob,
            executable: file.executable,
            rewrite,
            // See above: a dill never sets the dir-refusing flag.
            foreign_paths: false,
            stamp: Some(file.stamp),
        });
    }
    Ok((!files.is_empty()).then_some(DirScan {
        files,
        sources,
        skipped,
    }))
}

/// Whether a file name is one the test kernel cache claims: it contains
/// the `.cache.dill` marker (covers `<md5>.cache.dill` and its
/// `.track.dill` variant).
fn is_kernel_cache(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(".cache.dill"))
}

/// Enumerates every regular file below `hash_dir` for CAS ingest as a
/// [`DirScan`]: every file manifest-shaped, sorted by the hash-dir-relative
/// POSIX path, with the blob id (blake3 of the raw bytes), the executable
/// bit (`mode & 0o111 != 0`), the rewrite class, the foreign-path flag,
/// and the file's [`FileStamp`] at hash time.
///
/// A file is [`RewriteKind::PathText`] when its name marks it as
/// bookkeeping (`*.stamp`, `*.d`, `*.json`, `.filecache`) and its bytes
/// contain the WORKTREE root — the anchor a replay rewrites, wider than
/// the project root so workspace-sibling paths classify too — or the
/// dir's own `flutter_build/<name>` segment (a name that differs in
/// another tree must be rewritten there, A4). Escaped spellings of the
/// root are recognized as their own needles: depfiles escape spaces as
/// `\ `, JSON escapes backslashes (see [`escaped_forms`]). A payload
/// dill (`app.dill`, `kernel_blob.bin`) is classified through its
/// embedded `file://` URIs: the worktree-root URI occurring makes it
/// [`RewriteKind::DillUris`]. Unlike the test face there is no foreign
/// veto — a payload dill's constant pool legitimately carries synthetic
/// `file://` string literals (URI-shaped constants compiled into the
/// program), which the prefix-exact rewrite passes through untouched;
/// without the root URI it is a verbatim clone, never a dir-refusing
/// foreign record. Everything else — snapshots, shared libraries,
/// assets — is [`RewriteKind::None`] whatever it contains, and only
/// bookkeeping files and payload dills are ever searched. The search
/// covers the whole file: up to 1 MiB in the buffer that is hashed,
/// beyond that while the file streams through the hash, so a root
/// mentioned only in the middle of a large file is found at no extra
/// I/O.
///
/// A bookkeeping file over the materializer's rewrite limit is skipped
/// with a counted reason (see [`ScanSkips::oversize_rewrite`]): the
/// entry never carries a file every hit would refuse to rewrite.
/// Symlinks and non-UTF-8 names are skipped and counted the same way
/// instead of failing the scan.
///
/// The same pass classifies every absolute-path token against
/// `anchors`: one under neither the worktree root nor a cache root sets
/// [`ManifestFile::foreign_paths`], and the dir holding it is refused
/// by cross-tree replays (within-tree replay is unaffected — the paths
/// are valid where they were recorded).
///
/// `prior` is the dir record of the entry currently stored under the
/// ingest key whose replay name is this dir; `placed` the files this
/// invocation's materialization placed into it. A file whose identity
/// either record proves (see [`reused_identity`]) rides along as a
/// manifest reference without being read — flutter's content validation
/// touches stamp files on every warm run, and re-hashing a ~100 MB
/// payload that provably was not written is pure overhead. Everything
/// else is hashed (in parallel) and becomes an ingest source.
///
/// # Errors
///
/// [`BcacheError::Invalid`] for a relative worktree anchor;
/// [`BcacheError::Io`] when reading fails.
pub fn scan_entry_files(
    hash_dir: &Path,
    anchors: &PathAnchors,
    prior: Option<&DirRecord>,
    placed: Option<&[PlacedFile]>,
    blob_exists: &(dyn Fn(&BlobId) -> bool + Sync),
) -> Result<DirScan> {
    if !anchors.worktree_root.is_absolute() {
        return Err(BcacheError::Invalid(format!(
            "worktree root {} is not absolute",
            anchors.worktree_root.display()
        )));
    }
    // The rewrite needles' bytes: the worktree anchor, plus the dir's
    // own `flutter_build/<name>` segment when the dir is a config-hash
    // dir (its name embeds the ingest tree and must be rewritten in
    // another tree) — each in its escaped spellings (depfiles escape
    // spaces, JSON backslashes). Owned, so per-file `Finder`s can
    // borrow them.
    let mut needle_bytes = vec![
        anchors
            .worktree_root
            .as_os_str()
            .as_encoded_bytes()
            .to_vec(),
    ];
    if let Some(name) = hash_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| is_config_hash(name))
    {
        needle_bytes.push(format!("flutter_build/{name}").into_bytes());
    }
    let escaped: Vec<Vec<u8>> = needle_bytes
        .iter()
        .flat_map(|plain| {
            escaped_forms(plain)
                .into_iter()
                .filter(|escaped| escaped.as_slice() != plain.as_slice())
        })
        .collect();
    needle_bytes.extend(escaped);
    // The payload face's worktree-rootedness needle: the URI prefix a
    // kernel dill embeds for sources under the ingest root.
    let root_uri = {
        let mut needle = b"file://".to_vec();
        needle.extend_from_slice(anchors.worktree_root.as_os_str().as_encoded_bytes());
        needle.push(b'/');
        needle
    };
    let mut skipped = ScanSkips::default();
    let mut scanned = Vec::new();
    collect_files(hash_dir, &mut scanned, &mut skipped)?;
    scanned.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
    // A bookkeeping file over the rewrite limit can never materialize
    // as recorded: leave it out of the entry — flutter rewrites the
    // file itself on the next run.
    let oversize = |file: &ScannedFile| {
        file.bookkeeping && file.size > crate::materialize::REWRITE_LIMIT as u64
    };
    skipped.oversize_rewrite += scanned.iter().filter(|file| oversize(file)).count() as u64;
    scanned.retain(|file| !oversize(file));
    let scanned: Vec<(ManifestFile, Option<(PathBuf, BlobId)>)> = scanned
        .into_par_iter()
        .map(|file| {
            if let Some((blob, rewrite, foreign_paths)) =
                reused_identity(&file, prior, placed, blob_exists)
            {
                return Ok((
                    ManifestFile {
                        path: PathBuf::from(&file.relative),
                        blob,
                        executable: file.executable,
                        rewrite,
                        foreign_paths,
                        stamp: Some(file.stamp),
                    },
                    None,
                ));
            }
            let mut scan = if file.bookkeeping {
                Some(ByteScan::Bookkeeping {
                    needles: needle_bytes
                        .iter()
                        .map(|needle| memmem::Finder::new(needle))
                        .collect(),
                    foreign: ForeignPathScan::new(anchors),
                })
            } else if file.payload_dill {
                Some(ByteScan::DillUris {
                    foreign: ForeignUriScan::new(anchors),
                    root_uri: Some(memmem::Finder::new(&root_uri)),
                })
            } else {
                None
            };
            let (blob, mentions, foreign) = digest(&file.absolute, file.size, scan.as_mut())?;
            let manifest = ManifestFile {
                path: PathBuf::from(&file.relative),
                blob,
                executable: file.executable,
                // A payload dill rewrites cross-tree when the worktree
                // root URI occurs. The test face's foreign veto is
                // deliberately absent here: a real payload dill's
                // constant pool carries synthetic `file://` literals
                // (URI-shaped string constants compiled into the
                // program) that are data, not references, and the
                // prefix-exact rewrite passes them through untouched —
                // one literal must not cost the dill its rewriter.
                // Without the root URI it stays the verbatim clone it
                // always was — never a dir-refusing foreign record.
                rewrite: if file.bookkeeping && mentions {
                    RewriteKind::PathText
                } else if file.payload_dill && mentions {
                    RewriteKind::DillUris
                } else {
                    RewriteKind::None
                },
                foreign_paths: file.bookkeeping && foreign,
                stamp: Some(file.stamp),
            };
            Ok((manifest, Some((file.absolute, blob))))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut files = Vec::with_capacity(scanned.len());
    let mut sources = Vec::new();
    for (manifest, source) in scanned {
        if let Some(source) = source {
            sources.push(source);
        }
        files.push(manifest);
    }
    Ok(DirScan {
        files,
        sources,
        skipped,
    })
}

/// Whether a file name is a payload dill whose embedded `file://` URIs
/// decide its replay class: the kernel payloads `flutter build`
/// produces (`app.dill`) and the web face's `kernel_blob.bin`.
fn is_payload_dill(name: &str) -> bool {
    matches!(name, "app.dill" | "kernel_blob.bin")
}

fn is_config_hash(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The newest mtime among the regular files below `dir`; symlinks are not
/// followed.
fn newest_file_mtime(dir: &Path) -> Result<Option<SystemTime>> {
    let mut newest: Option<SystemTime> = None;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let mtime = entry.metadata()?.modified()?;
                newest = Some(newest.map_or(mtime, |seen| seen.max(mtime)));
            }
        }
    }
    Ok(newest)
}

/// One regular file below a hash dir.
struct ScannedFile {
    /// POSIX path relative to the hash dir.
    relative: String,
    absolute: PathBuf,
    size: u64,
    executable: bool,
    /// Named like a bookkeeping file that may record absolute paths.
    bookkeeping: bool,
    /// Named like a payload dill whose embedded URIs decide its class.
    payload_dill: bool,
    /// Filesystem identity, computed from the metadata the walk already
    /// holds; the reuse fast path and the manifest stamps read it.
    stamp: FileStamp,
}

/// Walks `hash_dir` collecting every regular file below it. Files a
/// manifest cannot represent — a symlink or other non-regular file, a
/// non-UTF-8 name — are skipped and counted in `skips` (a named
/// reason, never a failed scan: one odd file must not cost the run its
/// whole ingest).
fn collect_files(
    hash_dir: &Path,
    files: &mut Vec<ScannedFile>,
    skips: &mut ScanSkips,
) -> Result<()> {
    let mut pending = vec![(hash_dir.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let absolute = entry.path();
            let Ok(name) = entry.file_name().into_string() else {
                skips.non_utf8_names += 1;
                continue;
            };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push((absolute, format!("{prefix}{name}/")));
            } else if kind.is_file() {
                let metadata = entry.metadata()?;
                files.push(ScannedFile {
                    bookkeeping: is_bookkeeping(&name),
                    payload_dill: is_payload_dill(&name),
                    relative: format!("{prefix}{name}"),
                    absolute,
                    size: metadata.len(),
                    executable: is_executable(&metadata),
                    stamp: stamp_of(&metadata),
                });
            } else {
                skips.non_regular += 1;
            }
        }
    }
    Ok(())
}

fn is_bookkeeping(name: &str) -> bool {
    name == ".filecache"
        || matches!(
            Path::new(name).extension().and_then(OsStr::to_str),
            Some("stamp" | "d" | "json")
        )
}

#[cfg(unix)]
fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &Metadata) -> bool {
    false
}

/// The blob id of the file at `path` and, when `scan` is given, what
/// its bytes classify as: whether a rewrite needle matched (the
/// rewrite-kind hint) and whether a foreign absolute path was found.
/// One pass serves the hash and both classifications, whole-read up to
/// [`WHOLE_READ_LIMIT`] and streamed in windows beyond it — the needle
/// search carries its tail between windows, the foreign scanners keep
/// their own sequential state, and each byte feeds them exactly once.
fn digest(path: &Path, size: u64, scan: Option<&mut ByteScan<'_>>) -> Result<(BlobId, bool, bool)> {
    if size <= WHOLE_READ_LIMIT as u64 {
        let data = fs::read(path)?;
        let blob = BlobId(*blake3::hash(&data).as_bytes());
        let (mentions, foreign) = match scan {
            None => (false, false),
            Some(scan) => {
                let mentions = scan.any_needle_in(&data);
                scan.feed_foreign(&data);
                (mentions, scan.foreign())
            }
        };
        return Ok((blob, mentions, foreign));
    }
    let Some(scan) = scan else {
        return Ok((file_blake3(path)?, false, false));
    };
    let carry = scan.max_needle_len().saturating_sub(1);
    let mut buf = vec![0u8; carry + WHOLE_READ_LIMIT];
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let (mut carried, mut mentions) = (0usize, false);
    loop {
        let read = file.read(&mut buf[carried..carried + WHOLE_READ_LIMIT])?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[carried..carried + read]);
        // The foreign scanners see only the fresh window: the carried
        // bytes ahead of it were already classified.
        scan.feed_foreign(&buf[carried..carried + read]);
        let filled = carried + read;
        mentions = scan.any_needle_in(&buf[..filled]) || mentions;
        carried = carry.min(filled);
        buf.copy_within(filled - carried..filled, 0);
    }
    Ok((
        BlobId(*hasher.finalize().as_bytes()),
        mentions,
        scan.foreign(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    const HASH_A: &str = "0123456789abcdef0123456789abcdef";
    const HASH_B: &str = "fedcba9876543210fedcba9876543210";

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture dir");
        }
        fs::write(path, bytes).expect("write fixture");
    }

    /// Anchors of a synthetic tree: the worktree root is the fixture
    /// root itself; the cache roots are empty (matching nothing, so
    /// tests that want them set them explicitly).
    fn anchors_for(worktree: &Path) -> PathAnchors {
        PathAnchors {
            worktree_root: worktree.to_path_buf(),
            pub_cache: PathBuf::new(),
            flutter_root: PathBuf::new(),
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
    }

    fn set_mtime(path: &Path, time: SystemTime) {
        File::open(path)
            .and_then(|file| file.set_modified(time))
            .expect("set mtime");
    }

    fn blob(bytes: &[u8]) -> BlobId {
        BlobId(*blake3::hash(bytes).as_bytes())
    }

    #[test]
    fn scan_classifies_bookkeeping_by_root_mention() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let root_text = root.to_str().expect("UTF-8 temp path");
        let hash_dir = root.join(".dart_tool").join("flutter_build").join(HASH_A);
        let stamp = format!(
            r#"{{"inputs":["{root_text}/lib/main.dart"],"outputs":["{root_text}/build/app.dill"]}}"#
        );
        let filecache = format!(
            r#"{{"version":2,"files":[{{"path":"{root_text}/lib/main.dart","hash":"6a93"}}]}}"#
        );
        let depfile = format!("{root_text}/build/app.dill: {root_text}/lib/main.dart\n");
        // A payload above the whole-read limit that embeds the root: never a
        // rewrite candidate, whatever it contains.
        let mut dill = vec![0x90u8; WHOLE_READ_LIMIT + 4096];
        dill[100..100 + root_text.len()].copy_from_slice(root_text.as_bytes());
        let fixtures: [(&str, &[u8]); 7] = [
            ("kernel_snapshot_program.stamp", stamp.as_bytes()),
            (".filecache", filecache.as_bytes()),
            ("kernel_snapshot_program.d", depfile.as_bytes()),
            ("gen_localizations.stamp", br#"{"inputs":[],"outputs":[]}"#),
            (
                "native_assets.json",
                br#"{"format-version":[1,0,0],"native-assets":{}}"#,
            ),
            ("app.dill", &dill),
            ("App.framework/App", b"\xcf\xfa\xed\xfe payload"),
        ];
        for (name, bytes) in fixtures {
            write(&hash_dir.join(name), bytes);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let app = hash_dir.join("App.framework").join("App");
            fs::set_permissions(app, fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        let scanned =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        let summary: Vec<(&str, RewriteKind, bool)> = scanned
            .files
            .iter()
            .map(|file| {
                (
                    file.path.to_str().expect("UTF-8"),
                    file.rewrite,
                    file.executable,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (".filecache", RewriteKind::PathText, false),
                ("App.framework/App", RewriteKind::None, cfg!(unix)),
                ("app.dill", RewriteKind::None, false),
                ("gen_localizations.stamp", RewriteKind::None, false),
                ("kernel_snapshot_program.d", RewriteKind::PathText, false),
                (
                    "kernel_snapshot_program.stamp",
                    RewriteKind::PathText,
                    false
                ),
                ("native_assets.json", RewriteKind::None, false),
            ]
        );
        for (file, (absolute, blob_id)) in scanned.files.iter().zip(&scanned.sources) {
            assert_eq!(absolute, &hash_dir.join(&file.path));
            assert_eq!(&file.blob, blob_id);
            assert_eq!(file.blob, blob(&fs::read(absolute).expect("read back")));
            assert!(file.stamp.is_some(), "build-face files record stamps");
        }
        assert_eq!(
            scanned.sources.len(),
            scanned.files.len(),
            "no prior: every file hashed"
        );
    }

    #[test]
    fn scan_streams_large_bookkeeping_files_fully() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let root_bytes = root.to_str().expect("UTF-8 temp path").as_bytes();
        let hash_dir = root.join(HASH_B);
        let size = 2 * WHOLE_READ_LIMIT + 4096;
        // The root only in the middle, far from both ends.
        let mut middle = vec![b' '; size];
        middle[size / 2..size / 2 + root_bytes.len()].copy_from_slice(root_bytes);
        // The root spanning the boundary between the first two reads.
        let mut straddle = vec![b' '; size];
        let start = WHOLE_READ_LIMIT - 3;
        straddle[start..start + root_bytes.len()].copy_from_slice(root_bytes);
        let plain = vec![b'x'; size];
        write(&hash_dir.join("middle.json"), &middle);
        write(&hash_dir.join("straddle.stamp"), &straddle);
        write(&hash_dir.join("plain.d"), &plain);

        let scanned =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        let summary: Vec<(&str, RewriteKind, BlobId)> = scanned
            .files
            .iter()
            .map(|file| (file.path.to_str().expect("UTF-8"), file.rewrite, file.blob))
            .collect();
        assert_eq!(
            summary,
            [
                ("middle.json", RewriteKind::PathText, blob(&middle)),
                ("plain.d", RewriteKind::None, blob(&plain)),
                ("straddle.stamp", RewriteKind::PathText, blob(&straddle)),
            ]
        );
    }

    #[test]
    fn scan_rejects_relative_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = scan_entry_files(
            dir.path(),
            &PathAnchors {
                worktree_root: PathBuf::from("relative/root"),
                pub_cache: PathBuf::new(),
                flutter_root: PathBuf::new(),
            },
            None,
            None,
            &|_| true,
        )
        .expect_err("relative");
        assert!(matches!(err, BcacheError::Invalid(_)), "{err}");
    }

    #[test]
    fn dir_touched_since_reads_dir_and_contained_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path().join("test_cache");
        assert!(!dir_touched_since(&cache, at(0)).expect("missing dir"));
        write(
            &cache.join("build").join("ab.cache.dill.track.dill"),
            b"dill",
        );
        set_mtime(
            &cache.join("build").join("ab.cache.dill.track.dill"),
            at(100),
        );
        set_mtime(&cache.join("build"), at(50));
        set_mtime(&cache, at(50));
        assert!(dir_touched_since(&cache, at(0)).expect("touched"));
        assert!(dir_touched_since(&cache, at(100)).expect("file carries touch"));
        assert!(!dir_touched_since(&cache, at(101)).expect("stale"));
    }

    /// The test-cache scan: only `*.cache.dill*` files, only when
    /// touched, `None` on missing/empty/no-dill dirs.
    #[test]
    fn scan_test_cache_filters_dills_and_gates_on_touch() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let cache = root.join("build").join("test_cache");
        // Missing dir: nothing.
        assert!(
            scan_test_cache(&cache, at(0), &anchors_for(root), None, None, &|_| true)
                .expect("scan")
                .is_none(),
            "a missing dir ingests nothing"
        );

        // The live layout: build/test_cache/build/<md5>.cache.dill.track.dill
        // plus foreign files that must not ride along.
        write(&cache.join("build").join("ab.cache.dill"), b"kernel bytes");
        write(
            &cache.join("build").join("cd.cache.dill.track.dill"),
            b"tracked kernel bytes",
        );
        write(&cache.join("build").join("lock"), b"transient");
        set_mtime(&cache.join("build").join("ab.cache.dill"), at(100));
        set_mtime(
            &cache.join("build").join("cd.cache.dill.track.dill"),
            at(120),
        );
        set_mtime(&cache.join("build").join("lock"), at(110));
        set_mtime(&cache.join("build"), at(50));
        set_mtime(&cache, at(50));

        let scanned = scan_test_cache(&cache, at(0), &anchors_for(root), None, None, &|_| true)
            .expect("scan")
            .expect("files");
        let names: Vec<&str> = scanned
            .files
            .iter()
            .map(|file| file.path.to_str().expect("UTF-8"))
            .collect();
        assert_eq!(
            names,
            ["build/ab.cache.dill", "build/cd.cache.dill.track.dill"]
        );
        for file in &scanned.files {
            assert_eq!(
                file.rewrite,
                RewriteKind::DillUris,
                "a dill rewrites URIs cross-tree"
            );
            assert_eq!(
                file.blob,
                blob(&fs::read(cache.join(&file.path)).expect("read"))
            );
            assert!(file.stamp.is_some(), "every dill records its stamp");
        }
        assert_eq!(scanned.sources.len(), 2, "no prior: both dills hashed");

        // Untouched since the run started: nothing (the run wrote no
        // fresh dill — an untouched dir must not re-ingest).
        assert!(
            scan_test_cache(&cache, at(121), &anchors_for(root), None, None, &|_| true)
                .expect("scan")
                .is_none(),
            "an untouched dir ingests nothing"
        );

        // A touched dir holding no kernel-cache file: nothing.
        let bare = root.join("build2").join("test_cache");
        write(&bare.join("build").join("outputs.json"), b"{}");
        set_mtime(&bare.join("build").join("outputs.json"), at(200));
        assert!(
            scan_test_cache(&bare, at(0), &anchors_for(root), None, None, &|_| true)
                .expect("scan")
                .is_none(),
            "no dill means no ingest"
        );
    }

    /// The fast path: a dill that is the literal same file the prior
    /// entry stamped reuses the recorded blob without reading a byte.
    #[test]
    fn scan_test_cache_reuses_same_file_without_hashing() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let cache = root.join("test_cache");
        let dill = cache.join("build").join("ab.cache.dill.track.dill");
        write(&dill, b"test kernel bytes");
        set_mtime(&dill, at(100));
        set_mtime(&cache.join("build"), at(50));
        set_mtime(&cache, at(50));

        let first = scan_test_cache(&cache, at(0), &anchors_for(root), None, None, &|_| true)
            .expect("scan")
            .expect("files");
        assert_eq!(first.sources.len(), 1, "no prior: the dill is hashed");
        assert_eq!(first.sources[0].0, dill);
        assert_eq!(first.files[0].blob, blob(b"test kernel bytes"));

        // The next post-run scan over the unchanged file: the recorded
        // blob rides along and the file is never a source.
        let prior = prior_dir(first.files.clone());
        let second = scan_test_cache(
            &cache,
            at(0),
            &anchors_for(root),
            Some(&prior),
            None,
            &|_| true,
        )
        .expect("scan")
        .expect("files");
        assert!(second.sources.is_empty(), "no re-hash, no re-read");
        assert_eq!(second.files, first.files, "blob and stamp reused verbatim");
    }

    /// The hard line: a REPLACED file — new inode — re-hashes even when
    /// size and mtime collide with the recorded stamp.
    #[test]
    fn scan_test_cache_rehashes_replaced_file_despite_colliding_identity() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let cache = root.join("test_cache");
        let dill = cache.join("build").join("ab.cache.dill.track.dill");
        write(&dill, b"kernel v1");
        set_mtime(&dill, at(100));
        set_mtime(&cache.join("build"), at(50));
        set_mtime(&cache, at(50));

        let first = scan_test_cache(&cache, at(0), &anchors_for(root), None, None, &|_| true)
            .expect("scan")
            .expect("files");
        let stamp = first.files[0].stamp.expect("stamp");

        // Replace the file with same-size, same-mtime, different bytes:
        // only the inode can tell them apart.
        fs::remove_file(&dill).expect("remove");
        write(&dill, b"kernel v2");
        set_mtime(&dill, UNIX_EPOCH + Duration::from_nanos(stamp.mtime_ns));
        let metadata = fs::metadata(&dill).expect("metadata");
        assert_eq!(metadata.len(), stamp.size, "fixture: sizes collide");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(metadata.ino(), stamp.inode, "fixture: a fresh inode");
        }

        let prior = prior_dir(first.files.clone());
        let second = scan_test_cache(
            &cache,
            at(0),
            &anchors_for(root),
            Some(&prior),
            None,
            &|_| true,
        )
        .expect("scan")
        .expect("files");
        assert_eq!(second.sources.len(), 1, "a replaced file always re-hashes");
        assert_eq!(second.files[0].blob, blob(b"kernel v2"));
        assert_ne!(second.files[0].blob, first.files[0].blob);

        // The inode leg in isolation: a recorded stamp that matches
        // size, mtime, and device but names another inode never reuses.
        let mut colliding = stamp_of(&fs::metadata(&dill).expect("metadata"));
        colliding.inode = colliding.inode.wrapping_add(1);
        let mut files = second.files.clone();
        files[0].stamp = Some(colliding);
        let prior = prior_dir(files);
        let third = scan_test_cache(
            &cache,
            at(0),
            &anchors_for(root),
            Some(&prior),
            None,
            &|_| true,
        )
        .expect("scan")
        .expect("files");
        assert_eq!(third.sources.len(), 1, "an inode mismatch alone re-hashes");
    }

    /// A moved mtime on the same inode re-hashes; so do a legacy prior
    /// without a stamp and a recorded blob the store no longer holds.
    #[test]
    fn scan_test_cache_rehashes_on_mtime_stamp_gap_and_missing_blob() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let cache = root.join("test_cache");
        let dill = cache.join("build").join("ab.cache.dill.track.dill");
        write(&dill, b"kernel bytes");
        set_mtime(&dill, at(100));
        set_mtime(&cache.join("build"), at(50));
        set_mtime(&cache, at(50));
        let first = scan_test_cache(&cache, at(0), &anchors_for(root), None, None, &|_| true)
            .expect("scan")
            .expect("files");

        // The write frontend_server does every run: same inode, new
        // mtime.
        set_mtime(&dill, at(999));
        let prior = prior_dir(first.files.clone());
        let second = scan_test_cache(
            &cache,
            at(0),
            &anchors_for(root),
            Some(&prior),
            None,
            &|_| true,
        )
        .expect("scan")
        .expect("files");
        assert_eq!(second.sources.len(), 1, "a moved mtime re-hashes");
        assert_eq!(
            second.files[0].blob, first.files[0].blob,
            "same content, same blob"
        );

        // A legacy manifest (no stamp) never matches.
        set_mtime(&dill, at(100));
        let mut legacy = first.files.clone();
        legacy[0].stamp = None;
        let prior = prior_dir(legacy);
        let third = scan_test_cache(
            &cache,
            at(0),
            &anchors_for(root),
            Some(&prior),
            None,
            &|_| true,
        )
        .expect("scan")
        .expect("files");
        assert_eq!(third.sources.len(), 1, "a stampless prior record re-hashes");

        // A stamp match whose blob gc has removed must not be reused.
        let prior = prior_dir(first.files.clone());
        let fourth = scan_test_cache(
            &cache,
            at(0),
            &anchors_for(root),
            Some(&prior),
            None,
            &|_| false,
        )
        .expect("scan")
        .expect("files");
        assert_eq!(fourth.sources.len(), 1, "a missing recorded blob re-hashes");
    }

    /// A prior record of the dir, as the orchestrator passes it (the
    /// scan reads only its files; the name fields are inert here).
    fn prior_dir(files: Vec<ManifestFile>) -> DirRecord {
        DirRecord {
            name_observed: "build/test_cache".to_owned(),
            name_rule: crate::types::NameRule::TestKernelCache,
            name_defines: BTreeMap::new(),
            name_output_dir: String::new(),
            files,
        }
    }

    /// One placed-file record, as the materializer hands it to POST.
    fn placed_file(path: &str, stamp: FileStamp, blob: BlobId, verbatim: bool) -> PlacedFile {
        PlacedFile {
            path: PathBuf::from(path),
            stamp,
            blob,
            rewrite: RewriteKind::None,
            foreign_paths: false,
            verbatim,
        }
    }

    /// The build-face prior fast path: a file the stored entry stamped
    /// is re-referenced without reading a byte; a stampless (legacy)
    /// record never matches.
    #[test]
    fn scan_entry_files_reuses_prior_stamp_without_hashing() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let hash_dir = root.join(HASH_A);
        write(&hash_dir.join("a.stamp"), b"{}");
        write(&hash_dir.join("app.dill"), b"payload");

        let first =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        assert_eq!(first.sources.len(), 2, "no prior: both files hashed");
        assert!(first.files.iter().all(|file| file.stamp.is_some()));

        // The warm re-scan over unchanged files: prior stamps match, no
        // bytes read, manifest references carried verbatim.
        let prior = prior_dir(first.files.clone());
        let second = scan_entry_files(&hash_dir, &anchors_for(root), Some(&prior), None, &|_| true)
            .expect("scan");
        assert!(second.sources.is_empty(), "no re-hash, no re-read");
        assert_eq!(
            second.files, first.files,
            "blobs and stamps reused verbatim"
        );

        // A legacy manifest (no stamp) never matches for that file.
        let mut legacy = first.files.clone();
        legacy[0].stamp = None; // "a.stamp", the first of the sorted pair
        let prior = prior_dir(legacy);
        let third = scan_entry_files(&hash_dir, &anchors_for(root), Some(&prior), None, &|_| true)
            .expect("scan");
        assert_eq!(third.sources.len(), 1, "only the stamped record can reuse");
        assert_eq!(
            third.files[0].blob, first.files[0].blob,
            "still hashed to the same content"
        );
    }

    /// The placed fast path and its honesty rule: a verbatim placement
    /// with a matching stamp re-references its blob; a MISMATCHING
    /// placed stamp means the run rewrote the file — hashed even when a
    /// prior-entry stamp from an older ingest also matches.
    #[test]
    fn scan_entry_files_placed_identity_wins_and_suppresses_stale_prior() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let hash_dir = root.join(HASH_A);
        let dill = hash_dir.join("app.dill");
        write(&dill, b"payload v2");
        let on_disk = stamp_of(&fs::metadata(&dill).expect("metadata"));

        // The placement was verbatim and the run left the file alone:
        // the placed blob rides along, nothing is read.
        let placed = vec![placed_file(
            "app.dill",
            on_disk,
            blob(b"placed bytes"),
            true,
        )];
        let scanned = scan_entry_files(&hash_dir, &anchors_for(root), None, Some(&placed), &|_| {
            true
        })
        .expect("scan");
        assert!(scanned.sources.is_empty());
        assert_eq!(scanned.files[0].blob, blob(b"placed bytes"));
        assert_eq!(
            scanned.files[0].stamp,
            Some(on_disk),
            "the fresh stamp is recorded"
        );

        // The run rewrote the file after placement: the placed stamp no
        // longer matches. A prior record claiming the CURRENT identity
        // (with wrong content) must not be consulted — the file hashes.
        let stale_prior = prior_dir(vec![ManifestFile {
            path: PathBuf::from("app.dill"),
            blob: blob(b"stale prior bytes"),
            executable: false,
            rewrite: RewriteKind::None,
            foreign_paths: false,
            stamp: Some(on_disk),
        }]);
        let placed = vec![placed_file(
            "app.dill",
            FileStamp {
                mtime_ns: on_disk.mtime_ns.saturating_sub(1),
                ..on_disk
            },
            blob(b"placed bytes"),
            true,
        )];
        let scanned = scan_entry_files(
            &hash_dir,
            &anchors_for(root),
            Some(&stale_prior),
            Some(&placed),
            &|_| true,
        )
        .expect("scan");
        assert_eq!(
            scanned.sources.len(),
            1,
            "a rewritten file hashes despite the colliding prior stamp"
        );
        assert_eq!(scanned.files[0].blob, blob(b"payload v2"));
    }

    /// A non-verbatim placement (a cross-tree rewrite changed the bytes)
    /// and a placement whose blob gc has removed both fall back to
    /// hashing even on a stamp match.
    #[test]
    fn scan_entry_files_rehashes_nonverbatim_placed_and_missing_blob() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let hash_dir = root.join(HASH_A);
        let dill = hash_dir.join("app.dill");
        write(&dill, b"payload");
        let on_disk = stamp_of(&fs::metadata(&dill).expect("metadata"));

        let placed = vec![placed_file(
            "app.dill",
            on_disk,
            blob(b"rewritten bytes"),
            false,
        )];
        let scanned = scan_entry_files(&hash_dir, &anchors_for(root), None, Some(&placed), &|_| {
            true
        })
        .expect("scan");
        assert_eq!(
            scanned.sources.len(),
            1,
            "placed bytes are not the blob's bytes: hashed"
        );
        assert_eq!(scanned.files[0].blob, blob(b"payload"));

        let placed = vec![placed_file("app.dill", on_disk, blob(b"gc'd bytes"), true)];
        let scanned = scan_entry_files(&hash_dir, &anchors_for(root), None, Some(&placed), &|_| {
            false
        })
        .expect("scan");
        assert_eq!(
            scanned.sources.len(),
            1,
            "a missing recorded blob re-hashes"
        );
    }

    /// Files a manifest cannot represent are skipped with named
    /// counts — one symlink or odd name never costs the run its whole
    /// ingest (the pre-P3 behavior aborted the scan on them).
    #[cfg(unix)]
    #[test]
    fn scan_skips_unrepresentable_files_with_named_counts() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("a.stamp"), b"{}");
        std::os::unix::fs::symlink("a.stamp", dir.path().join("b.stamp")).expect("symlink");
        // A name that is not valid UTF-8 (a raw byte sequence no
        // manifest path can carry). macOS's kernel refuses to create
        // such a name at all, so the skip only materializes where the
        // write lands.
        let mut expected = ScanSkips {
            non_regular: 1,
            ..ScanSkips::default()
        };
        if fs::write(
            dir.path()
                .join(std::ffi::OsStr::from_bytes(b"\xff\xfe.stamp")),
            b"{}",
        )
        .is_ok()
        {
            expected.non_utf8_names = 1;
        }

        let scanned = scan_entry_files(dir.path(), &anchors_for(dir.path()), None, None, &|_| true)
            .expect("the scan keeps going");
        assert_eq!(scanned.skipped, expected);
        let names: Vec<&str> = scanned
            .files
            .iter()
            .map(|file| file.path.to_str().expect("UTF-8"))
            .collect();
        assert_eq!(names, ["a.stamp"], "only the regular file is recorded");
    }

    /// A bookkeeping file over the rewrite limit is refused at INGEST
    /// with a counted reason: recorded, it would fail every hit's
    /// rewrite; left out, the rest of the dir caches and flutter
    /// rewrites the file itself.
    #[test]
    fn scan_skips_oversize_bookkeeping_with_counted_reason() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("app.dill"), b"payload");
        let mut huge = Vec::new();
        huge.resize(crate::materialize::REWRITE_LIMIT + 1, b' ');
        fs::write(dir.path().join("kernel_snapshot_program.d"), &huge).expect("write huge");

        let scanned = scan_entry_files(dir.path(), &anchors_for(dir.path()), None, None, &|_| true)
            .expect("scan");
        assert_eq!(scanned.skipped.oversize_rewrite, 1);
        assert_eq!(scanned.skipped.total(), 1);
        let names: Vec<&str> = scanned
            .files
            .iter()
            .map(|file| file.path.to_str().expect("UTF-8"))
            .collect();
        assert_eq!(names, ["app.dill"], "the dir's other files are kept");

        // A payload of any size stays eligible (it streams, never
        // rewrites as text): the app.dill above is recorded — and a
        // non-bookkeeping file over the limit would be too.
        assert_eq!(scanned.files[0].rewrite, RewriteKind::None);
    }

    /// Depfiles spell a root containing spaces in escaped form
    /// (`\ `): the escaped spelling classifies the file as a rewrite
    /// candidate and does not mark it foreign — the tree with a space
    /// in its root caches and replays like any other.
    #[test]
    fn scan_matches_the_depfiles_escaped_root_spelling() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path().join("work tree");
        let root_text = root.to_str().expect("UTF-8 path");
        let escaped = root_text.replace(' ', "\\ ");
        let hash_dir = root.join(HASH_A);
        let depfile = format!("{escaped}/build/app.dill: {escaped}/lib/main.dart\n");
        write(
            &hash_dir.join("kernel_snapshot_program.d"),
            depfile.as_bytes(),
        );
        // The plain spelling in the same dir still classifies too.
        write(
            &hash_dir.join("outputs.json"),
            format!(r#"{{"out":["{root_text}/build/app.dill"]}}"#).as_bytes(),
        );

        let scanned =
            scan_entry_files(&hash_dir, &anchors_for(&root), None, None, &|_| true).expect("scan");
        let by_name = |name: &str| {
            scanned
                .files
                .iter()
                .find(|file| file.path == Path::new(name))
                .unwrap_or_else(|| panic!("no {name}"))
        };
        assert_eq!(
            by_name("kernel_snapshot_program.d").rewrite,
            RewriteKind::PathText,
            "the escaped root is a rewrite needle"
        );
        assert!(
            !by_name("kernel_snapshot_program.d").foreign_paths,
            "the escaped path token classifies as carried, not foreign"
        );
        assert_eq!(
            by_name("outputs.json").rewrite,
            RewriteKind::PathText,
            "the plain spelling still classifies"
        );
    }

    /// The payload face of the dill classification (A19, reworked by the
    /// #4069 finding): an `app.dill` (or `kernel_blob.bin`) that embeds
    /// worktree-rooted `file://` URIs ingests as
    /// [`RewriteKind::DillUris`] — the existing cross-tree rewrite
    /// applies to it — EVEN when the constant pool also carries
    /// synthetic `file://` literals (URI-shaped strings compiled into
    /// every real program, which the prefix-exact rewrite passes
    /// through). No root URI at all leaves it the verbatim clone it
    /// always was, never a dir-refusing foreign record.
    #[test]
    fn scan_classifies_payload_dills_by_worktree_uris() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let root_text = root.to_str().expect("UTF-8 path");
        let hash_dir = root.join(HASH_A);
        write(
            &hash_dir.join("app.dill"),
            format!(
                "file://{root_text}/lib/main.dart kernel \
                 file:///pubs/hosted/pub.dev/widgets-1.0.0/lib/widgets.dart \
                 file:///path/to/foo file:///root/path/a/b.dart"
            )
            .as_bytes(),
        );
        write(
            &hash_dir.join("kernel_blob.bin"),
            format!("file://{root_text}/lib/main.dart blob").as_bytes(),
        );
        write(
            &hash_dir.join("elsewhere.dill"),
            format!("file://{root_text}/lib/main.dart").as_bytes(),
        );
        // A dill whose only URIs sit outside every anchor.
        write(
            &hash_dir.join("sub").join("app.dill"),
            b"file:///elsewhere/lib/main.dart file:///path/to/foo",
        );

        let scanned =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        let kind_of = |path: &str| {
            scanned
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path}"))
                .rewrite
        };
        // The worktree URI decides; cache-root and synthetic literals
        // ride along without a veto.
        assert_eq!(kind_of("app.dill"), RewriteKind::DillUris);
        assert_eq!(kind_of("kernel_blob.bin"), RewriteKind::DillUris);
        // A dill by another name is not the payload face: no scan, no
        // classification, whatever it embeds.
        assert_eq!(kind_of("elsewhere.dill"), RewriteKind::None);
        // A payload with no worktree URI stays the verbatim clone — and
        // never flags the dir.
        let foreign = scanned
            .files
            .iter()
            .find(|file| file.path == Path::new("sub/app.dill"))
            .expect("nested payload");
        assert_eq!(foreign.rewrite, RewriteKind::None);
        assert!(!foreign.foreign_paths);
    }

    #[test]
    fn find_touched_hash_dirs_lists_all_since_sorted_by_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("flutter_build");
        assert_eq!(
            find_touched_hash_dirs(&root, at(0)).expect("missing root"),
            Vec::<(String, SystemTime)>::new()
        );
        let a_cache = root.join(HASH_A).join(".filecache");
        let b_cache = root.join(HASH_B).join(".filecache");
        let b_nested = root.join(HASH_B).join("App.framework").join("App");
        write(&a_cache, b"{}");
        write(&b_cache, b"{}");
        write(&b_nested, b"app");
        set_mtime(&a_cache, at(100));
        set_mtime(&b_cache, at(200));
        set_mtime(&b_nested, at(300));
        // Names that are not lowercase 32-hex are ignored however new.
        for name in [
            "not-a-config-hash/.filecache",
            "ABCDEF0123456789ABCDEF0123456789/.filecache",
            "0123456789abcdef0123456789abcde/.filecache",
            "dart_plugin_registrant.dart",
        ] {
            write(&root.join(name), b"{}");
            set_mtime(&root.join(name), at(900));
        }
        // Directory mtimes last: creating the files above moved them.
        set_mtime(&root.join(HASH_A), at(50));
        set_mtime(&root.join(HASH_B), at(50));

        let picked = |since| find_touched_hash_dirs(&root, since).expect("scan");
        // HASH_A < HASH_B as strings ('0' < 'f'), so the name sort lists
        // A first; touch times do not order the result.
        assert_eq!(picked(at(150)), vec![(HASH_B.to_owned(), at(300))]);
        assert_eq!(
            picked(at(0)),
            vec![(HASH_A.to_owned(), at(100)), (HASH_B.to_owned(), at(300))]
        );
        assert_eq!(picked(at(301)), Vec::<(String, SystemTime)>::new());
    }

    #[test]
    fn find_touched_hash_dirs_takes_dir_and_contained_file_mtimes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // One dir counts as touched through its own mtime (300 over a
        // 100-old file), the other through a contained file (250 over a
        // 50-old dir) — the max of the two drives the touch time.
        let (by_dir, by_file) = (
            "ffffffffffffffffffffffffffffffff",
            "00000000000000000000000000000000",
        );
        write(&root.join(by_dir).join("outputs.json"), b"[]");
        write(&root.join(by_file).join("outputs.json"), b"[]");
        set_mtime(&root.join(by_dir).join("outputs.json"), at(100));
        set_mtime(&root.join(by_file).join("outputs.json"), at(250));
        set_mtime(&root.join(by_dir), at(300));
        set_mtime(&root.join(by_file), at(50));
        assert_eq!(
            find_touched_hash_dirs(root, at(0)).expect("scan"),
            vec![(by_file.to_owned(), at(250)), (by_dir.to_owned(), at(300)),]
        );
        // Only the dir touched at 300 — through its own mtime — survives
        // a `since` past the other's file-carried 250.
        assert_eq!(
            find_touched_hash_dirs(&root, at(275)).expect("scan"),
            vec![(by_dir.to_owned(), at(300))]
        );
    }

    /// The A1/A6 classification: an absolute path under the worktree
    /// root or a cache root is replayable; anything else marks the file
    /// foreign; relative paths and `//`-headed tokens never do.
    #[test]
    fn foreign_path_classification_covers_the_three_anchor_groups() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let root_text = root.to_str().expect("UTF-8 temp path");
        let hash_dir = root.join(".dart_tool").join("flutter_build").join(HASH_A);
        // Fictional machine-local roots: absolute cache paths are
        // tree-independent, so invented spellings serve the prefix
        // classification without touching any real location.
        let anchors = PathAnchors {
            worktree_root: root.to_path_buf(),
            pub_cache: PathBuf::from("/fake/pub-cache"),
            flutter_root: PathBuf::from("/fake/flutter-sdk"),
        };
        let fixtures: [(&str, String); 6] = [
            (
                "worktree-sibling.stamp",
                // A workspace-sibling path package: under the worktree
                // root but OUTSIDE the project dir of this hash dir.
                format!(r#"[{{"inputs":["{root_text}/../packages/tools/lib/a.dart"]}}]"#),
            ),
            (
                "pub-cache.d",
                format!("{root_text}/out: /fake/pub-cache/hosted/x-1.0.0/lib/x.dart\n"),
            ),
            (
                "sdk.stamp",
                format!(r#"{{"input":"/fake/flutter-sdk/bin/cache/artifacts/i.png"}}"#),
            ),
            (
                // An escaped space truncates the token after `/opt/my\`;
                // a foreign head stays foreign either way.
                "foreign-escaped.d",
                format!("{root_text}/out: /opt/my\\ dir/dep.dart\n"),
            ),
            ("foreign.stamp", r#"["/opt/other/clone/lib/a.dart"]"#.to_owned()),
            (
                "relative-and-urls.stamp",
                r#"{"main":"lib/main.dart","url":"https://example.com/a","cdn":"//cdn.example.com/x","repo":"/"}"#
                    .to_owned(),
            ),
        ];
        for (name, text) in fixtures {
            write(&hash_dir.join(name), text.as_bytes());
        }

        let scanned = scan_entry_files(&hash_dir, &anchors, None, None, &|_| true).expect("scan");
        let flag = |name: &str| {
            scanned
                .files
                .iter()
                .find(|file| file.path == Path::new(name))
                .unwrap_or_else(|| panic!("no {name}"))
                .foreign_paths
        };
        assert!(!flag("worktree-sibling.stamp"), "under the worktree root");
        assert!(!flag("pub-cache.d"), "under the pub cache root");
        assert!(!flag("sdk.stamp"), "under the flutter root");
        assert!(flag("foreign-escaped.d"), "a foreign path stays foreign");
        assert!(flag("foreign.stamp"), "outside every root");
        assert!(
            !flag("relative-and-urls.stamp"),
            "relative paths, URLs, and a lone slash are not foreign"
        );
        // Byte-prefix boundaries: a path that merely STARTS WITH the
        // root's bytes is not under it.
        let sibling_root = format!("{root_text}X/lib/main.dart");
        write(&hash_dir.join("prefix-trap.stamp"), sibling_root.as_bytes());
        let scanned = scan_entry_files(&hash_dir, &anchors, None, None, &|_| true).expect("scan");
        assert!(
            scanned
                .files
                .iter()
                .find(|file| file.path == Path::new("prefix-trap.stamp"))
                .expect("trap file")
                .foreign_paths,
            "a byte-prefix lookalike is foreign"
        );
    }

    /// The A4 classification: a bookkeeping file that mentions the
    /// dir's own `flutter_build/<name>` segment — even with no absolute
    /// root at all — is a rewrite candidate, because the name itself
    /// changes in another tree.
    #[test]
    fn own_dir_name_mention_classifies_path_text() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let hash_dir = root.join(".dart_tool").join("flutter_build").join(HASH_A);
        write(
            &hash_dir.join("relative.stamp"),
            format!("[\".dart_tool/flutter_build/{HASH_A}/app.dill\"]").as_bytes(),
        );
        write(&hash_dir.join("payload.bin"), b"payload");

        let scanned =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        let kind = |name: &str| {
            scanned
                .files
                .iter()
                .find(|file| file.path == Path::new(name))
                .unwrap_or_else(|| panic!("no {name}"))
                .rewrite
        };
        assert_eq!(kind("relative.stamp"), RewriteKind::PathText);
        assert_eq!(kind("payload.bin"), RewriteKind::None);
    }

    /// The streaming leg: a foreign path and an anchor mention inside
    /// a bookkeeping file larger than the whole-read limit — one
    /// straddling the window boundary, one in the final window —
    /// classify exactly as the whole-read path does.
    #[test]
    fn foreign_classification_survives_streaming_window_boundaries() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let root_text = root.to_str().expect("UTF-8 temp path");
        let hash_dir = root.join(".dart_tool").join("flutter_build").join(HASH_A);
        let foreign = b"/opt/straddling/foreign/dep.dart";
        let anchored = format!("{root_text}/late/mention.dart");
        let mut data = vec![b' '; WHOLE_READ_LIMIT + 4096];
        let mid = WHOLE_READ_LIMIT - (foreign.len() / 2);
        data[mid..mid + foreign.len()].copy_from_slice(foreign);
        let late = data.len() - anchored.len() - 8;
        data[late..late + anchored.len()].copy_from_slice(anchored.as_bytes());
        write(&hash_dir.join(".filecache"), &data);

        let scanned =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        let file = &scanned.files[0];
        assert!(file.foreign_paths, "the straddling foreign path is found");
        assert_eq!(file.rewrite, RewriteKind::PathText, "the late anchor too");
        // Deterministic: a second pass classifies identically.
        let again =
            scan_entry_files(&hash_dir, &anchors_for(root), None, None, &|_| true).expect("scan");
        assert_eq!(again.files, scanned.files);
    }

    /// The A2 classification for the test face: a dill whose `file://`
    /// URIs sit under the worktree root ingests as `DillUris` (the
    /// cross-tree rewriter's contract); one carrying a URI outside
    /// every anchor downgrades to a verbatim clone (`None`), and
    /// package:/dart: URIs never matter.
    #[test]
    fn dill_uri_classification_foreign_vs_anchored() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let root_text = root.to_str().expect("UTF-8 temp path");
        let cache = root.join("build").join("test_cache");
        // Binary framing that imitates a dill's length-prefixed strings:
        // the scheme match must survive arbitrary framing bytes, and a
        // URI ends at the first control byte.
        let write_dill = |name: &str, bodies: &[&[u8]]| {
            let mut bytes = Vec::new();
            for body in bodies {
                bytes.extend_from_slice(b"\x00\x01file://");
                bytes.extend_from_slice(body);
                bytes.push(0x00);
            }
            write(&cache.join("build").join(name), &bytes);
            set_mtime(&cache.join("build").join(name), at(100));
        };
        write_dill(
            "anchored.cache.dill.track.dill",
            &[
                format!("{root_text}/lib/main.dart").as_bytes(),
                // A package URI has no file scheme: never classified.
                b"package:app/main.dart",
            ],
        );
        write_dill(
            "foreign.cache.dill.track.dill",
            &[
                format!("{root_text}/test/a.dart").as_bytes(),
                b"/opt/elsewhere/test/b.dart",
            ],
        );
        set_mtime(&cache.join("build"), at(50));
        set_mtime(&cache, at(50));

        let scanned = scan_test_cache(&cache, at(0), &anchors_for(root), None, None, &|_| true)
            .expect("scan")
            .expect("files");
        let kind = |name: &str| {
            scanned
                .files
                .iter()
                .find(|file| file.path == Path::new(&format!("build/{name}")))
                .unwrap_or_else(|| panic!("no {name}"))
                .rewrite
        };
        assert_eq!(
            kind("anchored.cache.dill.track.dill"),
            RewriteKind::DillUris
        );
        assert_eq!(kind("foreign.cache.dill.track.dill"), RewriteKind::None);
        assert!(
            scanned.files.iter().all(|file| !file.foreign_paths),
            "the dill face encodes its classification in the kind alone"
        );
    }
}
