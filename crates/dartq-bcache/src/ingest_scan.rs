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

use std::ffi::OsStr;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use memchr::memmem;
use rayon::prelude::*;

use crate::error::{BcacheError, Result};
use crate::fsutil::file_blake3;
use crate::types::{BlobId, ManifestFile, RewriteKind};

/// Files up to this size are read whole, one buffer serving both the hash
/// and the root search; larger bookkeeping files stream through windows of
/// this size.
const WHOLE_READ_LIMIT: usize = 1024 * 1024;

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

/// Enumerates every regular file below `hash_dir` for CAS ingest, as
/// `(absolute path, ManifestFile)` sorted by the hash-dir-relative POSIX
/// path, with the blob id (blake3 of the raw bytes), the executable bit
/// (`mode & 0o111 != 0`) and the rewrite class.
///
/// A file is [`RewriteKind::PathText`] when its name marks it as
/// bookkeeping (`*.stamp`, `*.d`, `*.json`, `.filecache`) and its bytes
/// contain `project_root`; everything else — dills, snapshots, shared
/// libraries, assets — is [`RewriteKind::None`] whatever it contains, and
/// only bookkeeping files are ever searched. The search covers the whole
/// file: up to 1 MiB in the buffer that is hashed, beyond that while the
/// file streams through the hash, so a root mentioned only in the middle of
/// a large file is found at no extra I/O. Escaped spellings of the root
/// (depfiles escape spaces as `\ `, JSON escapes backslashes) are not
/// recognized.
///
/// `project_root` must be absolute and spelled exactly as flutter spells it
/// in its bookkeeping files: the working directory flutter_tools ran in.
///
/// # Errors
///
/// [`BcacheError::Invalid`] for a relative `project_root`;
/// [`BcacheError::Unsupported`] for an entry a manifest cannot represent (a
/// symlink or other non-regular file, a non-UTF-8 name);
/// [`BcacheError::Io`] when reading fails.
pub fn scan_entry_files(
    hash_dir: &Path,
    project_root: &Path,
) -> Result<Vec<(PathBuf, ManifestFile)>> {
    if !project_root.is_absolute() {
        return Err(BcacheError::Invalid(format!(
            "project root {} is not absolute",
            project_root.display()
        )));
    }
    let root = memmem::Finder::new(project_root.as_os_str().as_encoded_bytes());
    let mut files = Vec::new();
    collect_files(hash_dir, &mut files)?;
    files.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
    files
        .into_par_iter()
        .map(|file| {
            let (blob, mentions_root) =
                digest(&file.absolute, file.size, file.bookkeeping.then_some(&root))?;
            let manifest = ManifestFile {
                path: PathBuf::from(file.relative),
                blob,
                executable: file.executable,
                rewrite: if mentions_root {
                    RewriteKind::PathText
                } else {
                    RewriteKind::None
                },
            };
            Ok((file.absolute, manifest))
        })
        .collect()
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
}

fn collect_files(hash_dir: &Path, files: &mut Vec<ScannedFile>) -> Result<()> {
    let mut pending = vec![(hash_dir.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let absolute = entry.path();
            let Ok(name) = entry.file_name().into_string() else {
                return Err(BcacheError::Unsupported(format!(
                    "{}: file name is not UTF-8",
                    absolute.display()
                )));
            };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push((absolute, format!("{prefix}{name}/")));
            } else if kind.is_file() {
                let metadata = entry.metadata()?;
                files.push(ScannedFile {
                    bookkeeping: is_bookkeeping(&name),
                    relative: format!("{prefix}{name}"),
                    absolute,
                    size: metadata.len(),
                    executable: is_executable(&metadata),
                });
            } else {
                return Err(BcacheError::Unsupported(format!(
                    "{}: not a regular file or directory",
                    absolute.display()
                )));
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

/// The blob id of the file at `path` and, when `root` is given, whether
/// its bytes contain the root.
fn digest(path: &Path, size: u64, root: Option<&memmem::Finder<'_>>) -> Result<(BlobId, bool)> {
    if size <= WHOLE_READ_LIMIT as u64 {
        let data = fs::read(path)?;
        let found = root.is_some_and(|root| root.find(&data).is_some());
        return Ok((BlobId(*blake3::hash(&data).as_bytes()), found));
    }
    let Some(root) = root else {
        return Ok((file_blake3(path)?, false));
    };
    // One streaming pass hashes every byte; the last `needle - 1` bytes of
    // each window are carried into the next so that a root spanning two
    // reads is still found.
    let carry = root.needle().len().saturating_sub(1);
    let mut buf = vec![0u8; carry + WHOLE_READ_LIMIT];
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let (mut carried, mut found) = (0, false);
    loop {
        let read = file.read(&mut buf[carried..carried + WHOLE_READ_LIMIT])?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[carried..carried + read]);
        let filled = carried + read;
        found = found || root.find(&buf[..filled]).is_some();
        carried = carry.min(filled);
        buf.copy_within(filled - carried..filled, 0);
    }
    Ok((BlobId(*hasher.finalize().as_bytes()), found))
}

#[cfg(test)]
mod tests {
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

        let scanned = scan_entry_files(&hash_dir, root).expect("scan");
        let summary: Vec<(&str, RewriteKind, bool)> = scanned
            .iter()
            .map(|(_, file)| {
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
        for (absolute, file) in &scanned {
            assert_eq!(absolute, &hash_dir.join(&file.path));
            assert_eq!(file.blob, blob(&fs::read(absolute).expect("read back")));
        }
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

        let scanned = scan_entry_files(&hash_dir, root).expect("scan");
        let summary: Vec<(&str, RewriteKind, BlobId)> = scanned
            .iter()
            .map(|(_, file)| (file.path.to_str().expect("UTF-8"), file.rewrite, file.blob))
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
        let err = scan_entry_files(dir.path(), Path::new("relative/root")).expect_err("relative");
        assert!(matches!(err, BcacheError::Invalid(_)), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn scan_rejects_symlinks() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("a.stamp"), b"{}");
        std::os::unix::fs::symlink("a.stamp", dir.path().join("b.stamp")).expect("symlink");
        let err = scan_entry_files(dir.path(), dir.path()).expect_err("symlink");
        assert!(matches!(err, BcacheError::Unsupported(_)), "{err}");
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
        assert_eq!(
            picked(at(150)),
            vec![(HASH_B.to_owned(), at(300))]
        );
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
            vec![
                (by_file.to_owned(), at(250)),
                (by_dir.to_owned(), at(300)),
            ]
        );
        // Only the dir touched at 300 — through its own mtime — survives
        // a `since` past the other's file-carried 250.
        assert_eq!(
            find_touched_hash_dirs(root, at(275)).expect("scan"),
            vec![(by_dir.to_owned(), at(300))]
        );
    }
}
