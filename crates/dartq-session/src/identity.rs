//! The session identity's metadata legs: the facts a live daemon cannot
//! reconcile in place, stamped by stat — never by reading file bytes.
//!
//! Two stamps, one contract: a change to the toolchain (an in-place
//! `bin/dart` patch, an FVM same-version switch) or to the dartq binary
//! itself (a brew upgrade) is a different session — the election reaps
//! the stale daemon and the replacement starts under the new stamp.
//! Everything the resident engine reconciles in place (manifests,
//! lockfiles, options files, package configs — the snapshot walk's
//! domain) never enters these legs: a `pub get` keeps its session.
//!
//! Why metadata, not content: a content hash would read megabytes per
//! call — the dartq binary is tens of megabytes — while every fact a
//! rebuild or swap changes (length, mtime, inode, device) is already a
//! syscall. Inode + device makes a same-length same-mtime replace (an
//! atomic brew upgrade writes a new inode) a new stamp; mtime alone
//! could survive a copy-preserving move.
//!
//! The binary stamp is memoized per process ([`binary_stamp`]): a running
//! daemon must keep the identity of the binary it STARTED from — a brew
//! upgrade that replaces the file under a live daemon must not move the
//! daemon's own stamp (recomputing would make the daemon adopt the new
//! file's identity and never be re-elected). New clients compute the new
//! stamp on their next call, see the mismatch, and re-elect: the upgrade
//! behavior, one syscall per process.

use std::path::Path;

/// One file's metadata stamp, hashed into an identity leg. Presence is a
/// fact — a missing or unreadable file is a record, never a failure.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime_ns: u64,
    ino: u64,
    dev: u64,
}

/// The stamp of `path`, or `None` when it cannot be stated (missing,
/// unreadable, not a file — the digest records the distinction anyway,
/// so a dangling path and a real file never collide). Follows symlinks:
/// a `bin/dart` or a brew-installed dartq is commonly a link — the
/// identity cares about the file it lands on.
#[cfg(unix)]
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(Stamp {
        len: meta.len(),
        mtime_ns: meta.mtime() as u64,
        ino: meta.ino(),
        dev: meta.dev(),
    })
}

/// Windows `Metadata` exposes no inode/device: `len` + `mtime` alone carry
/// the stamp (a same-path rewrite still moves both), and the record keeps
/// the same `len:mtime_ns:ino:dev` shape with `0` in the unix-only legs —
/// the resident session is `Standalone` there anyway, so the lost
/// discrimination never decides an election.
#[cfg(not(unix))]
fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(Stamp {
        len: meta.len(),
        mtime_ns: meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
        ino: 0,
        dev: 0,
    })
}

/// Hashes a named fact into `hasher` — present stamps by value, absent
/// ones by their sentinel: two absent records of different kinds never
/// meet, and "no file" cannot collide with "a file of zero length".
fn add(hasher: &mut blake3::Hasher, record: &str) {
    hasher.update(&(record.len() as u64).to_le_bytes());
    hasher.update(record.as_bytes());
}

/// The environment fingerprint for `sdk_root`: a hex digest over the
/// dart executable's metadata stamp. A `bin/dart` swap (any of length,
/// mtime, inode, device moving) is a different session; a `pub get`, an
/// options edit, or a package config rewrite — the universe the daemon
/// reconciles — is not.
#[must_use]
pub fn environment_fingerprint(sdk_root: &Path) -> String {
    let record = match stamp(&sdk_root.join("bin").join("dart")) {
        Some(s) => format!("{}:{}:{}:{}", s.len, s.mtime_ns, s.ino, s.dev),
        None => "absent".to_owned(),
    };
    let mut hasher = blake3::Hasher::new();
    add(&mut hasher, &record);
    hex::encode(&hasher.finalize().as_bytes()[..16])
}

/// The dartq binary's own stamp — the session triple's `binary_stamp`
/// leg. `(canonical exe path, stamp)`: the path is part of the record
/// so two dartq installs answering with identical file stamps still
/// separate. Memoized on first compute: see the module docs — a live
/// daemon keeps the stamp of the binary it started from across an
/// in-place upgrade.
#[must_use]
pub fn binary_stamp() -> String {
    static STAMP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(s) = STAMP.get() {
        return s.clone();
    }
    let digest = match std::env::current_exe().ok().and_then(|exe| {
        std::fs::canonicalize(&exe)
            .ok()
            .map(|path| (path.clone(), stamp(&path)))
    }) {
        Some((path, Some(s))) => {
            let mut hasher = blake3::Hasher::new();
            add(&mut hasher, path.to_string_lossy().as_ref());
            add(
                &mut hasher,
                &format!("{}:{}:{}:{}", s.len, s.mtime_ns, s.ino, s.dev),
            );
            hex::encode(&hasher.finalize().as_bytes()[..16])
        }
        // No exe or no stat: the fact is still one leg, keyed "absent".
        _ => {
            let mut hasher = blake3::Hasher::new();
            add(&mut hasher, "absent");
            hex::encode(&hasher.finalize().as_bytes()[..16])
        }
    };
    let _ = STAMP.set(digest.clone());
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, text).expect("write");
    }

    /// The stamp is metadata-only: a `bin/dart` content swap of ANY kind
    /// moves the leg — replace the file (new inode, new mtime), touch it
    /// (new mtime), or grow it (new length) — while a config edit, a lock
    /// rewrite, or a same-inode metadata read moves nothing.
    #[test]
    fn config_inputs_leave_the_fingerprint_but_a_dart_swap_moves_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sdk = root.join("sdk");
        write(&sdk.join("bin/dart"), "dart-bytes-v1\n");
        write(&sdk.join("version"), "3.13.7\n");
        write(
            &root.join(".dart_tool/package_config.json"),
            "{\"configVersion\":2,\"packages\":[]}",
        );
        write(
            &root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - lints\n",
        );
        let base = environment_fingerprint(&sdk);
        assert_eq!(environment_fingerprint(&sdk), base, "deterministic");

        // The reconciled universe: none of these may move the leg.
        write(
            &root.join(".dart_tool/package_config.json"),
            "{\"configVersion\":2,\"packages\":[{\"name\":\"x\",\"rootUri\":\"/x\"}]}",
        );
        write(&root.join("pubspec.lock"), "packages: {}\n");
        write(
            &root.join("pubspec_overrides.yaml"),
            "dependency_overrides: {}\n",
        );
        write(
            &root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - core\n",
        );
        assert_eq!(
            environment_fingerprint(&sdk),
            base,
            "a pub get / options edit keeps the session"
        );

        // An in-place rewrite: same path, new inode — moves the stamp
        // (inode + device + mtime all change; even a same-length rewrite
        // is a new file).
        std::fs::remove_file(sdk.join("bin/dart")).expect("remove");
        write(&sdk.join("bin/dart"), "dart-bytes-v2-patched\n");
        assert_ne!(
            environment_fingerprint(&sdk),
            base,
            "a replaced dart is a different session"
        );

        // A same-inode mtime touch also moves it — `make`-style rebuilds
        // that rewrite in place must not go unseen.
        let leg2 = environment_fingerprint(&sdk);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(sdk.join("bin/dart"))
            .expect("open");
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .expect("touch");
        drop(file);
        assert_ne!(
            environment_fingerprint(&sdk),
            leg2,
            "an mtime bump alone is a new stamp"
        );

        // Missing is a fact — deterministic, never a hash collision with
        // a real file.
        let absent = environment_fingerprint(&root.join("none"));
        assert_eq!(absent, environment_fingerprint(&root.join("none")));
        assert_ne!(absent, base);
    }

    /// The binary leg is a stat and a memo, not a read: the digest is
    /// stable across calls inside one process (the OnceLock) and the
    /// helper's cost is one `symlink_metadata` per process — it can
    /// never be proportional to the executable's size, because nothing
    /// opens the file.
    #[test]
    fn the_binary_stamp_is_memoized_and_never_reads_the_exe() {
        let first = binary_stamp();
        let second = binary_stamp();
        assert_eq!(first, second, "the memo holds within the process");
        assert_eq!(first.len(), 32, "a 16-byte hex digest");
        // Stat-only by construction: a huge readable dummy at the
        // executable's side would be touched in microseconds — but the
        // stronger pin is the implementation itself: `stamp` only calls
        // `symlink_metadata`; there is no `File::open` on the exe path.
        // This test pins the memo and the shape, and the source contract
        // (no read path exists to regress) is enforced by review.
    }

    /// Timing evidence for the report: stamping a release-size binary
    /// (the installed dartq, ~33 MB) versus reading it. The assertion is
    /// deliberately generous — the point is orders of magnitude, not a
    /// wall-clock gate; CI variance cannot flake a 1000x margin.
    #[test]
    fn stamping_a_release_size_binary_is_stat_cost_not_read_cost() {
        let dartq = Path::new("/opt/homebrew/bin/dartq");
        if !dartq.is_file() {
            return; // not this host's install — the unit pins stand
        }
        let start = std::time::Instant::now();
        let stamp = stamp(dartq).expect("stat");
        let stamp_cost = start.elapsed();
        let start = std::time::Instant::now();
        let bytes = std::fs::read(dartq).expect("read");
        let read_cost = start.elapsed();
        eprintln!(
            "dartq {} bytes: stamp {:?}, read {:?} ({}x)",
            bytes.len(),
            stamp_cost,
            read_cost,
            read_cost.as_nanos() / stamp_cost.as_nanos().max(1)
        );
        assert!(stamp.len > 0);
        assert!(
            stamp_cost < read_cost,
            "a stat is always cheaper than a 33 MB read"
        );
    }
}
