//! The per-tree placement record: which stored blob each file of a
//! materialized build dir renders, keyed by the file's identity —
//! `<project>/.dart_tool/dartq-bcache/placed/<entry-key>.json`.
//!
//! The entry manifest is the shared source every tree hits from, and
//! its file stamps name the files of the tree that ingested it. A tree
//! that materialized the entry holds files of its own identities
//! (fresh inodes and mtimes; cross-tree, re-rooted bytes too), which
//! those stamps cannot vouch for — so the post-run scan of the next run
//! in that tree re-hashed every output file once, and with several
//! worktrees sharing one entry every tree alternation paid that
//! re-hash and republished the manifest under its own identities (the
//! manifest ping-ponged between trees). The record keeps each tree's
//! identities beside the tree, where the manifest stays as ingested:
//! written by the placement (the fresh stamps of what it placed, from
//! which blob) and by every post-run ingest (the scan's view of the
//! tree, so the record never lags it), and consulted by the post-run
//! scan ahead of the manifest's stamps. A record hit means no read, no
//! hash, no classification for that file.
//!
//! The record is a memo of the same class as the manifest's stamp fast
//! path — `(identity → blob)`, exact by the stamp semantics
//! ([`crate::types::FileStamp`]: every write moves the ns mtime, a
//! replaced file gets a new inode) — never a correctness input: a stale
//! identity cannot match, the store is asked whether it still holds the
//! blob before any reference is made, and a missing or undecodable
//! record is simply ignored. Key-scoped: different build configs of one
//! tree are different entries, and a record for another key is never
//! consulted (the file name selects it, and the record names its key
//! inside for good measure).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::PlacedFile;

/// One tree's placement memo for one entry key.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PlacementRecord {
    /// The entry key the record belongs to (also its file name).
    pub key: String,
    pub dirs: Vec<PlacedDir>,
}

/// One dir of a record, under the name the post-run scan knows it by:
/// the hash-dir name of a `.dart_tool/flutter_build` dir, or the
/// project-relative path of a fixed dir (the test kernel cache, the
/// bundle asset dir).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PlacedDir {
    pub name: String,
    pub files: Vec<PlacedFile>,
}

/// The record file for `key` under `project_root`.
fn record_path(project_root: &Path, key: &str) -> PathBuf {
    project_root
        .join(".dart_tool")
        .join("dartq-bcache")
        .join("placed")
        .join(format!("{key}.json"))
}

/// The record for `key` in this tree; `None` when there is none, when
/// it does not decode, or when it names another key.
#[must_use]
pub fn load(project_root: &Path, key: &str) -> Option<PlacementRecord> {
    let bytes = fs::read(record_path(project_root, key)).ok()?;
    serde_json::from_slice::<PlacementRecord>(&bytes)
        .ok()
        .filter(|record| record.key == key)
}

/// Writes `record` as this tree's record for its key: tmp sibling plus
/// rename, so a reader never sees a torn record. A record whose bytes
/// already sit on disk is left alone.
///
/// # Errors
///
/// The underlying write; the caller treats it as best-effort (a lost
/// record costs the next run one re-hash, which rewrites it).
pub fn store(project_root: &Path, record: &PlacementRecord) -> io::Result<()> {
    let path = record_path(project_root, &record.key);
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    if fs::read(&path).is_ok_and(|prior| prior == bytes) {
        return Ok(());
    }
    let dir = path.parent().expect("record path has a parent");
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.dartq-tmp", record.key, std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// The record's dirs for `key` in this tree, or none (see [`load`]).
#[must_use]
pub fn known_dirs(project_root: &Path, key: &str) -> Vec<PlacedDir> {
    load(project_root, key).map_or_else(Vec::new, |record| record.dirs)
}

/// `placed` (what this run just placed, fill-only-missing) overlaid on
/// `known` (what the tree remembered): in a dir both know, this run's
/// file replaces the known file of the same path and the rest stay
/// (they were present, hence not placed, and their record is as good
/// as before); a dir only one side knows is kept whole. The result is
/// the tree's current knowledge, in the known order with new dirs
/// appended.
#[must_use]
pub fn overlay(mut known: Vec<PlacedDir>, placed: Vec<PlacedDir>) -> Vec<PlacedDir> {
    for fresh in placed {
        match known.iter_mut().find(|dir| dir.name == fresh.name) {
            Some(dir) => {
                for file in fresh.files {
                    match dir.files.iter_mut().find(|known| known.path == file.path) {
                        Some(known) => *known = file,
                        None => dir.files.push(file),
                    }
                }
            }
            None => known.push(fresh),
        }
    }
    known
}

/// `scanned` (a post-run scan's view, every file of each dir it
/// covered) laid over `known`: a scanned dir replaces the known dir of
/// its name whole — a file the scan did not see is gone from the tree
/// — and dirs the scan did not cover are kept. Same ordering as
/// [`overlay`].
#[must_use]
pub fn replace_dirs(mut known: Vec<PlacedDir>, scanned: Vec<PlacedDir>) -> Vec<PlacedDir> {
    for fresh in scanned {
        match known.iter_mut().find(|dir| dir.name == fresh.name) {
            Some(dir) => *dir = fresh,
            None => known.push(fresh),
        }
    }
    known
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::{BlobId, FileStamp, RewriteKind};

    fn file(path: &str, inode: u64, blob: u8) -> PlacedFile {
        PlacedFile {
            path: PathBuf::from(path),
            stamp: FileStamp {
                dev: 1,
                inode,
                size: 3,
                mtime_ns: 7,
            },
            blob: BlobId([blob; 32]),
            rewrite: RewriteKind::None,
            foreign_paths: false,
            on_disk: Some(BlobId([blob; 32])),
        }
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Round trip, atomic placement, and the two ways a record is not
    /// consulted: another key's file (never opened) and undecodable
    /// bytes (ignored). The key inside is checked too.
    #[test]
    fn roundtrip_and_ignores_foreign_or_corrupt_records() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let record = PlacementRecord {
            key: KEY.to_owned(),
            dirs: vec![PlacedDir {
                name: "build/flutter_assets".to_owned(),
                files: vec![
                    file("kernel_blob.bin", 11, 1),
                    file("AssetManifest.bin", 12, 2),
                ],
            }],
        };
        assert_eq!(load(root, KEY), None, "nothing recorded yet");
        store(root, &record).expect("store");
        assert_eq!(load(root, KEY).as_ref(), Some(&record));
        let dir = root.join(".dart_tool").join("dartq-bcache").join("placed");
        assert_eq!(
            fs::read_dir(&dir).expect("placed dir").count(),
            1,
            "no temp sibling survives the rename"
        );

        let other = "f".repeat(64);
        assert_eq!(
            load(root, &other),
            None,
            "another key's record is never consulted"
        );
        // A record whose file name and inner key disagree is ignored.
        fs::rename(
            dir.join(format!("{KEY}.json")),
            dir.join(format!("{other}.json")),
        )
        .expect("rename");
        assert_eq!(load(root, &other), None);
        fs::write(dir.join(format!("{KEY}.json")), b"{not json").expect("corrupt");
        assert_eq!(load(root, KEY), None, "an undecodable record is ignored");
    }

    /// The overlay rule: a fresh placement wins its path, known files it
    /// did not touch stay, unknown dirs are added, and nothing known
    /// yields the placement itself.
    #[test]
    fn overlay_merges_fresh_placements_over_the_known() {
        let known = vec![PlacedDir {
            name: "a".to_owned(),
            files: vec![file("x", 1, 1), file("y", 2, 2)],
        }];
        let fresh = vec![
            PlacedDir {
                name: "a".to_owned(),
                files: vec![file("y", 20, 3), file("z", 30, 4)],
            },
            PlacedDir {
                name: "b".to_owned(),
                files: vec![file("w", 40, 5)],
            },
        ];
        assert_eq!(overlay(Vec::new(), fresh.clone()), fresh);
        let merged = overlay(known, fresh);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "a");
        assert_eq!(
            merged[0].files,
            vec![file("x", 1, 1), file("y", 20, 3), file("z", 30, 4)]
        );
        assert_eq!(merged[1].name, "b");
        assert_eq!(merged[1].files, vec![file("w", 40, 5)]);
    }

    /// The post-run rule: a scanned dir replaces the known dir whole (a
    /// file the scan did not see is gone), uncovered dirs survive.
    #[test]
    fn replace_dirs_swaps_scanned_dirs_whole() {
        let known = vec![
            PlacedDir {
                name: "a".to_owned(),
                files: vec![file("x", 1, 1), file("y", 2, 2)],
            },
            PlacedDir {
                name: "b".to_owned(),
                files: vec![file("w", 3, 3)],
            },
        ];
        let scanned = vec![PlacedDir {
            name: "a".to_owned(),
            files: vec![file("y", 20, 4)],
        }];
        assert_eq!(replace_dirs(Vec::new(), scanned.clone()), scanned);
        let merged = replace_dirs(known, scanned);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[0].files,
            vec![file("y", 20, 4)],
            "x is gone with the dir"
        );
        assert_eq!(
            merged[1].files,
            vec![file("w", 3, 3)],
            "b survives untouched"
        );
    }
}
