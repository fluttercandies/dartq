//! Filesystem primitives for the build cache: clonefile-first copying,
//! bounded streaming copy, and streamed content hashing.
//!
//! Placement of store blobs into a project worktree must never use
//! hardlinks: flutter_tools rewrites some outputs in place, and a
//! hardlink would corrupt the shared store. `clonefile(2)` produces an
//! independent copy-on-write snapshot (measured on APFS: 4.4 GB
//! materializes in 0.379 s, orders of magnitude faster than a plain
//! copy), so it is the mandatory fast path on macOS, with a bounded
//! streaming copy as the universal fallback.

#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
#[cfg(unix)]
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::types::BlobId;

/// Fixed I/O buffer for every streaming path in this crate: 1 MiB.
/// All copies and hashes run through buffers of this size, so peak
/// memory stays flat regardless of entry size.
const CHUNK: usize = 1024 * 1024;

/// Outcome of [`clone_file`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneStatus {
    /// The destination was created as a copy-on-write clone
    /// (`clonefile(2)` on APFS).
    Cloned,
    /// Cloning was refused (cross-volume, filesystem without clone
    /// support, permission model, non-macOS target, ...) and the
    /// destination was reproduced with a bounded streaming copy.
    Copied,
}

/// Copy `src` to `dst`, creating or replacing `dst`.
///
/// On macOS the first attempt is `clonefile(2)`; on Linux it is the
/// `FICLONE` ioctl. If the clone attempt fails for any reason (EXDEV
/// across volumes, ENOTSUP on a filesystem without clone support, EPERM,
/// ...) the copy falls back to [`copy_stream`] — on Linux through the
/// `copy_file_range(2)` rung of the ladder below. `clonefile` refuses
/// to overwrite an existing destination, so EEXIST is handled by
/// removing `dst` and retrying the clone exactly once before falling
/// back. The fallback truncates an existing destination, so the final
/// state is always "dst holds src's bytes". `dst`'s parent directory
/// must exist.
pub fn clone_file(src: &Path, dst: &Path) -> io::Result<CloneStatus> {
    #[cfg(target_os = "macos")]
    {
        if clonefile_macos(src, dst).is_ok() {
            return Ok(CloneStatus::Cloned);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if ficlone_or_copy_linux(src, dst)? {
            return Ok(CloneStatus::Cloned);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        copy_stream(src, dst)?;
    }
    Ok(CloneStatus::Copied)
}

/// The Linux rungs between the clone and the plain read/write loop:
/// `FICLONE` first, then `copy_file_range(2)`, then [`copy_stream`].
/// Returns `true` when the FICLONE rung cloned the whole file.
///
/// Kernel API contract (written against the kernel headers, exercised
/// here only through the portable fallbacks — this workspace builds on
/// macOS):
///
/// * `FICLONE` (`_IOW(0x94, 9, int)`, Linux ≥ 2.6.38) turns an already
///   created regular destination file into a copy-on-write clone of the
///   source descriptor. It fails `EXDEV` across filesystems and
///   `EOPNOTSUPP`/`EINVAL` where the destination filesystem lacks
///   reflink support (ext4, NFS, tmpfs); every failure leaves the
///   destination untouched, so the copy rungs start from a clean
///   truncated file.
/// * `copy_file_range(2)` (Linux ≥ 4.5; cross-filesystem since 5.3)
///   copies inside the kernel — no user-space round trip, and possible
///   server-side copies on network filesystems. It is issued by number
///   through `syscall(2)`: the C library wrapper would bind the
///   versioned `copy_file_range@GLIBC_2.27` symbol — a binary that then
///   fails to start on an older glibc — and is missing from some C
///   libraries, while the syscall number exists on every Linux
///   architecture. It reports `EINVAL` for unsupported filesystem
///   pairs, `EPERM`/`EXDEV` on refusal, and `ENOSYS` where the kernel
///   (or a seccomp filter) lacks the call; those fall through to the
///   generic loop. Both descriptors advance in lockstep (offsets passed
///   as NULL), so a partial copy before an error is continued, not
///   restarted, by the read/write rung.
#[cfg(target_os = "linux")]
fn ficlone_or_copy_linux(src: &Path, dst: &Path) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;

    let reader = File::open(src)?;
    let writer = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;
    // SAFETY: ioctl(2) with FICLONE takes the source descriptor by
    // value as its third argument; both descriptors are owned by this
    // function and outlive the call. The return value decides the rung.
    let cloned = unsafe { libc::ioctl(writer.as_raw_fd(), libc::FICLONE, reader.as_raw_fd()) } == 0;
    if cloned {
        return Ok(true);
    }
    copy_range_then_read_write(&reader, &writer)?;
    Ok(false)
}

/// The `copy_file_range` rung, falling through to the buffered
/// read/write loop on the documented refusal errors. A regular-file
/// `EAGAIN` cannot occur without O_NONBLOCK, but is treated as a
/// refusal rather than an abort for safety. A 0 before any byte moved
/// falls through too: besides an empty source, some kernels answer 0
/// for a non-empty file they cannot copy in-kernel (procfs, overlayfs
/// on some releases), and one `read` of the next rung tells the two
/// apart.
#[cfg(target_os = "linux")]
fn copy_range_then_read_write(reader: &File, writer: &File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let mut copied_any = false;
    loop {
        // SAFETY: copy_file_range(2) takes both descriptors by value,
        // two NULL `loff_t *` offsets (each file's position then
        // advances by exactly the count returned — the partial-progress
        // bookkeeping below relies on nothing else), a `size_t` length
        // and `unsigned int` flags; every variadic argument is passed at
        // that kernel type, and nothing is borrowed past the call.
        let copied = unsafe {
            libc::syscall(
                libc::SYS_copy_file_range,
                reader.as_raw_fd(),
                std::ptr::null_mut::<libc::loff_t>(),
                writer.as_raw_fd(),
                std::ptr::null_mut::<libc::loff_t>(),
                CHUNK,
                0 as libc::c_uint,
            )
        };
        match copied {
            1.. => copied_any = true,
            0 if copied_any => return Ok(()),
            0 => break,
            _ => {
                let err = io::Error::last_os_error();
                match err.raw_os_error() {
                    Some(
                        libc::EINVAL
                        | libc::EPERM
                        | libc::EXDEV
                        | libc::ENOSYS
                        | libc::EOPNOTSUPP
                        | libc::EAGAIN,
                    ) => break,
                    _ => return Err(err),
                }
            }
        }
    }
    // The read/write rung continues from the descriptors' current
    // offsets — exactly where copy_file_range stopped.
    let mut reader = reader;
    let mut writer = writer;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        writer.write_all(&buf[..n])?;
    }
}

/// One clone attempt plus the single EEXIST remove-and-retry.
/// Any resulting error means "cloning refused or impossible" and is
/// reported to the caller, which falls back to a streaming copy.
#[cfg(target_os = "macos")]
fn clonefile_macos(src: &Path, dst: &Path) -> io::Result<()> {
    let csrc = cstring(src)?;
    let cdst = cstring(dst)?;
    // SAFETY: clonefile(2) reads the two NUL-terminated paths by pointer
    // and returns; nothing escapes, and the return value is checked on
    // every path. Flags 0 requests a plain clone.
    unsafe {
        if libc::clonefile(csrc.as_ptr(), cdst.as_ptr(), 0) == 0 {
            return Ok(());
        }
        if io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
            // clonefile never overwrites: clear the stale destination and
            // retry once. A removal failure is surfaced to the fallback.
            fs::remove_file(dst)?;
            if libc::clonefile(csrc.as_ptr(), cdst.as_ptr(), 0) == 0 {
                return Ok(());
            }
        }
        Err(io::Error::last_os_error())
    }
}

/// Converts a path to the C-string form clonefile requires. Interior
/// NUL bytes cannot exist in a valid path component, but the conversion
/// is fallible so it is mapped to an explicit error.
#[cfg(target_os = "macos")]
fn cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// Streaming copy with a 1 MiB buffer. The whole file is never held in
/// memory. Creates or truncates `dst`; `dst`'s parent directory must
/// exist. Returns the number of bytes copied.
pub fn copy_stream(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut reader = File::open(src)?;
    let mut writer = File::create(dst)?;
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    writer.flush()?;
    Ok(total)
}

/// Clone-then-hash: the ingest-side primitive that halves the I/O of
/// storing a file (audit A16: one copy-on-write clone plus one read
/// replaces the read+write of the plain copying path, ~152.8 ms per
/// 117.3 MiB ingest). The hash runs over `dst` — the bytes that landed —
/// so the digest vouches for exactly what the destination holds, never
/// for a source that changed underneath the copy. Returns the placement
/// status, the byte count, and that digest. The caller keeps its
/// scratch→rename publish discipline around `dst`.
pub fn clone_file_hashed(src: &Path, dst: &Path) -> io::Result<(CloneStatus, u64, BlobId)> {
    let status = clone_file(src, dst)?;
    let len = fs::metadata(dst)?.len();
    let digest = file_blake3(dst)?;
    Ok((status, len, digest))
}

/// The maximum number of scoped reader threads a parallel hash fan-out
/// spawns. Four saturates the I/O side of a materialization integrity
/// pass without contending with the store's own rayon pool.
#[cfg(unix)]
const HASH_WORKERS: usize = 4;

/// Hashes every path with up to [`HASH_WORKERS`] scoped threads,
/// results index-aligned with `paths`. blake3 streams at ~1.5 GiB/s per
/// core, but a restore's integrity check is otherwise one serialized
/// read chain (audit A15: 76.3 ms sequential vs 24.0 ms threaded for a
/// 117 MiB dir). A scoped thread fan-out over the file list is the
/// whole mechanism — rayon would drag a pool into every caller for no
/// gain at this width.
#[cfg(unix)]
pub(crate) fn hash_files_parallel(paths: &[PathBuf]) -> Vec<io::Result<BlobId>> {
    if paths.len() <= 1 {
        return paths.iter().map(|path| file_blake3(path)).collect();
    }
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, io::Result<BlobId>)>();
    std::thread::scope(|scope| {
        let next = &next;
        for _ in 0..HASH_WORKERS.min(paths.len()) {
            let tx = tx.clone();
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, AtomicOrdering::Relaxed);
                    if index >= paths.len() {
                        break;
                    }
                    // A send failure means the receiver is gone (a sibling
                    // worker poisoned nothing — results are independent).
                    let _ = tx.send((index, file_blake3(&paths[index])));
                }
            });
        }
        drop(tx);
    });
    let mut results: Vec<io::Result<BlobId>> =
        paths.iter().map(|_| io::Result::Err(unreached())).collect();
    for (index, result) in rx {
        results[index] = result;
    }
    results
}

/// The placeholder filling a parallel-hash slot before its (guaranteed)
/// result arrives; every index is claimed exactly once, so this never
/// escapes to a caller.
#[cfg(unix)]
fn unreached() -> io::Error {
    io::Error::new(io::ErrorKind::Other, "parallel hash slot unfilled")
}

/// An exclusive advisory lock serializing cache mutations on one
/// project tree: `.dart_tool/dartq-bcache.lock`, taken with `flock(2)`
/// around a hit's materialization and a run's post-ingest, so two
/// shims in the same tree — a terminal and an IDE — queue instead of
/// interleaving a placement with a scan (audit A14). The lock is held
/// by the open file description: dropping the guard closes the file
/// and releases it, and a crashed shim's descriptor dies with the
/// process, so a tree can never stay wedged.
#[cfg(unix)]
pub(crate) struct TreeLock {
    _file: File,
}

/// The contention poll interval: a blocking `flock` cannot be given a
/// deadline without SIGALRM gymnastics, so the wait is a non-blocking
/// probe at this granularity. The wait cap itself is the caller's
/// policy (the orchestrator holds it at ten minutes in production).
#[cfg(unix)]
const TREE_LOCK_POLL: Duration = Duration::from_millis(25);

#[cfg(unix)]
impl TreeLock {
    /// Takes the exclusive tree lock under `tree`'s `.dart_tool`,
    /// creating the directory and lock file as needed. `Ok(None)`: the
    /// lock was still held when the wait cap elapsed — the caller
    /// proceeds without the cache, never blocking the build.
    ///
    /// # Errors
    ///
    /// [`io::Error`] when the lock file cannot be created or `flock`
    /// fails with anything but `EWOULDBLOCK` (the caller's choice is
    /// then to proceed unsynchronized, which the per-process temp
    /// names keep safe from corruption).
    pub(crate) fn acquire(tree: &Path, wait: Duration) -> io::Result<Option<Self>> {
        use std::os::unix::io::AsRawFd;

        let dir = tree.join(".dart_tool");
        fs::create_dir_all(&dir)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("dartq-bcache.lock"))?;
        let deadline = Instant::now() + wait;
        loop {
            // SAFETY: flock(2) operates on the descriptor owned by
            // `file` for the duration of the call; LOCK_EX|LOCK_NB
            // returns immediately instead of sleeping in the kernel.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Some(TreeLock { _file: file }));
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(err);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(TREE_LOCK_POLL);
        }
    }
}

/// Bumps `path`'s mtime to now — the classic `touch(1)`. The store's
/// reuse signal: a blob an ingest references without rewriting gets its
/// mtime refreshed so gc's in-flight window (and its pre-unlink
/// re-verify) sees it as just-needed. An error is a real filesystem
/// problem and fails the ingest that asked for the touch.
///
/// The re-verify sees a touch only if it lands in a later mtime tick
/// than gc's scan recorded. The in-flight window makes that hold — only
/// mtimes older than the window reach the re-verify — unless the
/// filesystem's clock trails gc's by more than the window; then a
/// coarse-mtime filesystem (ext3, HFS+, some NFS) can hide a same-tick
/// touch, and gc may unlink a blob a manifest is about to reference: a
/// miss → rebuild, never corruption.
pub(crate) fn touch(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let cpath = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        // SAFETY: utimensat reads the NUL-terminated path by pointer and
        // returns; `times` = NULL sets atime and mtime to the current
        // time; flags 0 resolve the path as given against AT_FDCWD.
        if unsafe { libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), std::ptr::null(), 0) } == 0 {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    #[cfg(not(unix))]
    {
        // No utimensat: re-truncate to the same length, which the OS
        // records as a modification (Windows' SetEndOfFile moves the
        // last-write time even when the length is unchanged).
        let len = fs::metadata(path)?.len();
        fs::OpenOptions::new().write(true).open(path)?.set_len(len)
    }
}

/// The space of one volume, in bytes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VolumeSpace {
    /// What an unprivileged writer may still claim (`f_bavail`: the
    /// blocks reserved for root are not the cache's to fill).
    pub(crate) available: u64,
    /// The volume's full size (`f_blocks`).
    pub(crate) total: u64,
}

/// The [`VolumeSpace`] of the volume holding `path`, from `statvfs(3)`
/// (POSIX; the C library declares it on every unix target). Both counts
/// are in `f_frsize` units, multiplied out with saturation. There is
/// deliberately no `statfs` fallback: any failure is the caller's
/// signal to proceed without the measurement.
#[cfg(unix)]
pub(crate) fn volume_space(path: &Path) -> io::Result<VolumeSpace> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cpath = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    // SAFETY: all-zero is a valid `statvfs` (integer fields, padding
    // only as `MaybeUninit`).
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: statvfs(3) reads the NUL-terminated path and fills `stats`
    // through pointers that both outlive the call; `stats` is read only
    // after a zero return.
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stats) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // The field types are per-platform aliases (`u32` block counts on
    // macOS, `u64` on Linux), all unsigned and at most 64 bits wide.
    let fragment = stats.f_frsize as u64;
    Ok(VolumeSpace {
        available: (stats.f_bavail as u64).saturating_mul(fragment),
        total: (stats.f_blocks as u64).saturating_mul(fragment),
    })
}

/// No `statvfs` off unix: the space is unknown.
#[cfg(not(unix))]
pub(crate) fn volume_space(_path: &Path) -> io::Result<VolumeSpace> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "volume space needs statvfs",
    ))
}

/// Streamed blake3 of a file (1 MiB buffer). Used to verify blob
/// identity before materializing: a corrupted store blob must abort
/// the materialization rather than land in a project tree.
pub fn file_blake3(path: &Path) -> io::Result<crate::types::BlobId> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(crate::types::BlobId(*hasher.finalize().as_bytes()))
}

/// The filesystem identity of `metadata`: device and inode on unix,
/// size, and nanosecond mtime. See [`crate::types::FileStamp`] for why
/// an all-fields match is proof the file was not written since — the
/// shared identity semantics of the post-run scans and the
/// materializer's placed-file capture.
#[cfg(unix)]
pub(crate) fn stamp_of(metadata: &fs::Metadata) -> crate::types::FileStamp {
    use std::os::unix::fs::MetadataExt;
    crate::types::FileStamp {
        dev: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mtime_ns: mtime_ns(metadata),
    }
}

#[cfg(not(unix))]
pub(crate) fn stamp_of(metadata: &fs::Metadata) -> crate::types::FileStamp {
    crate::types::FileStamp {
        dev: 0,
        inode: 0,
        size: metadata.len(),
        mtime_ns: mtime_ns(metadata),
    }
}

/// mtime in nanoseconds since the epoch; 0 for a pre-epoch clock,
/// `u64::MAX` past the nanosecond range. Nanosecond resolution is what
/// the store's pre-unlink re-verify compares: a reuse touch landing in
/// the same whole second as a gc scan must still look different.
pub(crate) fn mtime_ns(metadata: &fs::Metadata) -> u64 {
    use std::time::UNIX_EPOCH;
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    #[test]
    fn clone_file_yields_identical_content() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data: Vec<u8> = (0..250_000u32).map(|i| (i % 251) as u8).collect();
        let src = write_file(tmp.path(), "src.bin", &data);
        let dst = tmp.path().join("dst.bin");
        let status = clone_file(&src, &dst).expect("clone_file");
        assert!(matches!(status, CloneStatus::Cloned | CloneStatus::Copied));
        assert_eq!(
            file_blake3(&src).expect("hash src"),
            file_blake3(&dst).expect("hash dst")
        );
    }

    #[test]
    fn clone_file_replaces_existing_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = write_file(tmp.path(), "src.bin", b"fresh clone bytes");
        let dst = write_file(
            tmp.path(),
            "dst.bin",
            b"stale destination bytes that differ",
        );
        let status = clone_file(&src, &dst).expect("clone_file over existing dst");
        assert!(matches!(status, CloneStatus::Cloned | CloneStatus::Copied));
        assert_eq!(std::fs::read(&dst).expect("read dst"), b"fresh clone bytes");
    }

    #[test]
    fn copy_stream_roundtrips_across_chunk_boundaries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // CHUNK + 3 bytes: exercises at least two read iterations and a
        // trailing partial chunk.
        let mut data = vec![7u8; CHUNK + 3];
        data[0] = 1;
        data[CHUNK + 2] = 9;
        let src = write_file(tmp.path(), "src.bin", &data);
        let dst = tmp.path().join("dst.bin");
        let copied = copy_stream(&src, &dst).expect("copy_stream");
        assert_eq!(copied, (CHUNK + 3) as u64);
        assert_eq!(std::fs::read(&dst).expect("read dst"), data);
    }

    #[test]
    fn file_blake3_matches_in_memory_hash() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let path = write_file(tmp.path(), "blob.bin", &data);
        let digest = file_blake3(&path).expect("file_blake3");
        assert_eq!(digest.0, *blake3::hash(&data).as_bytes());
    }

    /// A18's fallback chain on the generic path: a source no clone
    /// implementation accepts (a character device) must fall through to
    /// the plain copy rung and still produce a valid destination. On
    /// macOS this exercises clonefile→copy_stream; on Linux,
    /// FICLONE→copy_file_range→read/write all refuse or handle it.
    #[cfg(unix)]
    #[test]
    fn clone_file_falls_back_when_the_clone_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dst = tmp.path().join("zero.bin");
        let status = clone_file(Path::new("/dev/null"), &dst).expect("clone_file of /dev/null");
        assert_eq!(status, CloneStatus::Copied, "no clone rung takes a device");
        assert_eq!(dst.metadata().expect("stat dst").len(), 0);
    }

    /// A16's primitive: clone-then-hash lands byte-identical content
    /// with the digest of exactly those bytes, and the plain copy path
    /// lands the same digest. The clone mechanism is asserted, not
    /// timed: `CloneStatus::Cloned` means clonefile/FICLONE returned
    /// success — an atomic CoW clone with no hidden copy inside — which
    /// is the structural one-write-less win over copy-then-hash. A
    /// wall-clock beats-copy assertion lived here briefly and flapped
    /// under parallel test load (scheduler and page-cache noise invert
    /// both paths' timings); it proved nothing the mechanism check does
    /// not, so it is gone.
    #[cfg(unix)]
    #[test]
    fn clone_file_hashed_lands_identical_content_on_the_cow_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data: Vec<u8> = (0..6_000_000u32).map(|i| (i % 251) as u8).collect();
        let src = write_file(tmp.path(), "src.bin", &data);

        let (status, len, digest) =
            clone_file_hashed(&src, &tmp.path().join("clone.bin")).expect("clone_file_hashed");
        assert_eq!(len, data.len() as u64);
        assert_eq!(digest.0, *blake3::hash(&data).as_bytes());
        assert_eq!(
            fs::read(tmp.path().join("clone.bin")).expect("read clone"),
            data,
            "content is byte-identical"
        );

        let copy_dst = tmp.path().join("copy.bin");
        copy_stream(&src, &copy_dst).expect("copy_stream");
        let copied_digest = file_blake3(&copy_dst).expect("hash the copy");
        assert_eq!(copied_digest, digest);
        #[cfg(target_os = "macos")]
        assert_eq!(status, CloneStatus::Cloned, "same-volume APFS clone");
        #[cfg(not(target_os = "macos"))]
        assert!(matches!(status, CloneStatus::Cloned | CloneStatus::Copied));
    }

    /// A15: the scoped fan-out produces exactly the sequential digests,
    /// and on a multi-file MiB-scale fixture it is measurably faster
    /// than the serialized chain. The margin assertion is loose by
    /// design — it flakes on no machine with ≥2 cores and a warm cache.
    #[cfg(unix)]
    #[test]
    fn hash_files_parallel_matches_sequential_and_runs_faster() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut paths = Vec::new();
        for file in 0..12 {
            let data: Vec<u8> = (0..4_000_000u32).map(|i| (i % 249) as u8).collect();
            paths.push(write_file(
                tmp.path(),
                &format!("blob-{file:02}.bin"),
                &data,
            ));
        }

        let started = Instant::now();
        let sequential: Vec<_> = paths
            .iter()
            .map(|path| file_blake3(path).expect("hash"))
            .collect();
        let sequential_elapsed = started.elapsed();

        let started = Instant::now();
        let parallel = hash_files_parallel(&paths);
        let parallel_elapsed = started.elapsed();
        for (index, result) in parallel.into_iter().enumerate() {
            assert_eq!(result.expect("hash result"), sequential[index]);
        }
        assert!(
            parallel_elapsed.as_secs_f64() < sequential_elapsed.as_secs_f64() * 0.75,
            "parallel ({parallel_elapsed:?}) vs sequential ({sequential_elapsed:?})"
        );
    }

    /// The volume probe measures a real volume (free space never exceeds
    /// the size) and fails — the caller's fall-back signal — for a path
    /// that does not exist.
    #[cfg(unix)]
    #[test]
    fn volume_space_measures_the_volume_holding_a_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let space = volume_space(tmp.path()).expect("statvfs of a tempdir");
        assert!(space.total > 0, "a mounted volume has a size: {space:?}");
        assert!(space.available <= space.total, "{space:?}");
        let missing = volume_space(&tmp.path().join("missing")).expect_err("no such path");
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }

    /// A14: the tree lock serializes two acquirers — the second waits
    /// for the first's drop — and a capped wait degrades to `None`
    /// instead of blocking forever.
    #[cfg(unix)]
    #[test]
    fn tree_lock_serializes_and_degrades_on_contention() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tree = tmp.path().join("project");
        fs::create_dir_all(&tree).expect("mkdir tree");

        let first = TreeLock::acquire(&tree, Duration::from_millis(100))
            .expect("first acquire")
            .expect("uncontended lock taken");

        // Contended with a short cap: refuses while `first` is held.
        assert!(
            TreeLock::acquire(&tree, Duration::from_millis(150))
                .expect("contended acquire")
                .is_none(),
            "contention past the cap degrades to None"
        );

        // The second acquirer queues behind the first's release.
        let queued_tree = tree.clone();
        let releaser = std::thread::spawn(move || {
            let second = TreeLock::acquire(&queued_tree, Duration::from_secs(5))
                .expect("second acquire")
                .expect("lock after release");
            second
        });
        std::thread::sleep(Duration::from_millis(100));
        drop(first);
        let second = releaser.join().expect("queued acquirer");
        drop(second);

        // Re-acquirable immediately after both are gone.
        assert!(
            TreeLock::acquire(&tree, Duration::from_millis(100))
                .expect("re-acquire")
                .is_some()
        );
    }
}
