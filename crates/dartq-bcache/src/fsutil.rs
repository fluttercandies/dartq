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
use std::path::Path;

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
/// On macOS the first attempt is `clonefile(2)`; if it fails for any
/// reason (EXDEV across volumes, ENOTSUP on non-APFS filesystems, EPERM,
/// ...) the copy falls back to [`copy_stream`]. `clonefile` refuses to
/// overwrite an existing destination, so EEXIST is handled by removing
/// `dst` and retrying the clone exactly once before falling back. The
/// fallback truncates an existing destination, so the final state is
/// always "dst holds src's bytes". `dst`'s parent directory must exist.
pub fn clone_file(src: &Path, dst: &Path) -> io::Result<CloneStatus> {
    #[cfg(target_os = "macos")]
    {
        if clonefile_macos(src, dst).is_ok() {
            return Ok(CloneStatus::Cloned);
        }
    }
    copy_stream(src, dst)?;
    Ok(CloneStatus::Copied)
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
        assert_eq!(file_blake3(&src).expect("hash src"), file_blake3(&dst).expect("hash dst"));
    }

    #[test]
    fn clone_file_replaces_existing_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = write_file(tmp.path(), "src.bin", b"fresh clone bytes");
        let dst = write_file(tmp.path(), "dst.bin", b"stale destination bytes that differ");
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
}
