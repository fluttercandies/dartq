//! The cache key engine: the toolchain fingerprint and the coarse
//! config key.
//!
//! A key names one build in every worktree: the same toolchain, define
//! vector, target, and provenance file contents give the same key
//! wherever the project is checked out, and anything else gives a
//! different one. Provenance files are named by path-independent
//! [`FileIdentity`] values and keyed by content (blake3), never by
//! mtime: a touched but unchanged file keeps its key, and an edited
//! file with a forged mtime still changes it.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use rayon::join;
use serde::Deserialize;

use crate::error::{BcacheError, Result};
use crate::types::{
    BlobId, ClosureFile, FileIdentity, KeyInputs, ToolchainFingerprint, blob_id_hex,
};

/// First line of every key preimage. Any change to the canonical format
/// must change this line, which changes every key with it.
///
/// v3: `.dart_tool/package_config.json` left the closure — it exists
/// only after flutter's implicit `pub get`, so keying it split the
/// pre-build key of a fresh worktree from the key the entry is stored
/// under, and the headline use case never hit. The bump also retires
/// every v2 entry: v2 manifests predate the worktree-anchor replay
/// semantics, and a fresh key space guarantees no entry recorded under
/// the old classification rules is ever replayed under the new ones.
const FORMAT_LINE: &[u8] = b"format: bcache-key-v3\n";

/// Read buffer for streamed hashing: 1 MiB, the crate-wide streaming size.
const HASH_BUF: usize = 1 << 20;

/// Resolves the flutter SDK root from the path of a `flutter` binary,
/// following symlinks (an fvm shim resolves into
/// `<flutter-root>/bin/flutter`): the root is the parent of the resolved
/// binary's directory.
///
/// # Errors
///
/// [`BcacheError::NotFound`] if the binary does not exist,
/// [`BcacheError::Io`] if it cannot be resolved, and
/// [`BcacheError::Invalid`] if the resolved path has no grandparent.
pub fn flutter_root(flutter_bin: &Path) -> Result<PathBuf> {
    let resolved = fs::canonicalize(flutter_bin).map_err(|err| io_error(flutter_bin, err))?;
    resolved
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            BcacheError::Invalid(format!(
                "{} has no flutter root above it",
                resolved.display()
            ))
        })
}

/// Fingerprints the flutter toolchain at `flutter_root`, reading the
/// flutter_tools 3.47.1 layout:
///
/// * `engine_revision`: `bin/cache/engine-dart-sdk.stamp` (trimmed);
/// * `flutter_version`: `frameworkVersion` of
///   `bin/cache/flutter.version.json`;
/// * `platform_strong_dill`: blake3 of
///   `bin/cache/dart-sdk/lib/_internal/vm_platform_strong.dill`;
/// * `frontend_server_snapshot`: blake3 of
///   `bin/cache/dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot`
///   (the snapshot `flutter_tools` runs).
///
/// The two artifacts (tens of MB) are hashed concurrently, each streamed
/// through a 1 MiB buffer.
///
/// # Errors
///
/// [`BcacheError::NotFound`] naming a missing field's file,
/// [`BcacheError::Invalid`] for an unparsable version manifest, and
/// [`BcacheError::Io`] for any other read failure.
pub fn fingerprint_toolchain(flutter_root: &Path) -> Result<ToolchainFingerprint> {
    let cache = flutter_root.join("bin").join("cache");
    let engine_revision = read_trimmed(&cache.join("engine-dart-sdk.stamp"))?;
    let flutter_version = framework_version(&cache.join("flutter.version.json"))?;

    let sdk = cache.join("dart-sdk");
    let dill = sdk
        .join("lib")
        .join("_internal")
        .join("vm_platform_strong.dill");
    let frontend = sdk
        .join("bin")
        .join("snapshots")
        .join("frontend_server_aot.dart.snapshot");
    let (dill_id, frontend_id) = join(|| hash_artifact(&dill), || hash_artifact(&frontend));
    Ok(ToolchainFingerprint {
        engine_revision,
        flutter_version,
        platform_strong_dill: dill_id?,
        frontend_server_snapshot: frontend_id?,
    })
}

/// The define vector that feeds the key: `raw` minus the defines that are
/// provenance rather than build input. Exactly `FLUTTER_GIT_URL` drops;
/// every other define stays, read or not, so the key stays conservative
/// (deduplication belongs to the blob layer, never to the key).
///
/// flutter_tools 3.47.1 injects `FLUTTER_GIT_URL` from the SDK checkout's
/// remote (`repositoryUrl` of `bin/cache/flutter.version.json`), so it is
/// constant for one SDK install. It is still compiled into the kernel as
/// the `FlutterVersion.gitUrl` constant: a hit can carry another install's
/// remote URL there.
#[must_use]
pub fn filter_defines(raw: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    raw.iter()
        .filter(|(name, _)| name.as_str() != "FLUTTER_GIT_URL")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The cache key of `inputs`: the blake3 of [`preimage`] as 64
/// lowercase hex digits.
///
/// The key covers exactly what a tree carries before any build —
/// toolchain, filtered define vector, target, and the contents of the
/// provenance files the caller puts in `inputs.closure` (pubspec,
/// define files) — so a fresh worktree keys identically to the tree an
/// entry was ingested from. Two file families must never enter the
/// closure: a depfile-sourced closure exists only after a build has
/// run in that tree, and `.dart_tool/package_config.json` exists only
/// after flutter's implicit `pub get` — either one splits the
/// pre-build key from the stored key and the fresh-tree hit dies.
///
/// Safety of the coarseness: whatever the config key cannot see is
/// caught where flutter itself catches it — every build re-validates
/// the recorded input hashes of a build dir (stamps, `.filecache`) and
/// rebuilds on mismatch, and frontend_server invalidates a stale
/// initialize-from-dill kernel. That delegation is sound for a
/// replayed dir because the replay rules guarantee the recorded paths
/// resolve identically in the target tree: worktree-root paths are
/// rewritten to it, cache-root paths are machine-constant, and a dir
/// mentioning anything else is refused cross-tree (see
/// [`crate::types::PathAnchors`]). A stale hit therefore costs one
/// wasted materialization, never a wrong artifact.
#[must_use]
pub fn config_key(inputs: &KeyInputs) -> String {
    blake3::hash(&preimage(inputs)).to_hex().to_string()
}

/// The canonical serialization [`config_key`] hashes: line-oriented text
/// a human can audit and diff (paths contribute their raw bytes).
///
/// ```text
/// format: bcache-key-v3
/// toolchain.engine_revision <engine revision>
/// toolchain.flutter_version <flutter version>
/// toolchain.platform_strong_dill <hex>
/// toolchain.frontend_server_snapshot <hex>
/// target.command <command>
/// target.platform <platform>
/// target.mode <mode>
/// target.entry <entry>                 only when set
/// target.flavor <flavor>               only when set
/// target.extra_key_arg <arg>           one per arg, in argv order
/// define <name>=<value>                one per define, in name order
/// <order key> <content hex> <size>     one per closure file
/// ```
///
/// A closure file's order key is `T:<project-relative path>` or
/// `A:<absolute path>`. Closure lines are sorted bytewise by order key,
/// then content, then size, so input order never matters. In every
/// value, `\`, LF, and CR are written `\\`, `\n`, and `\r` (and `=` is
/// written `\=` in define names): each field stays on its own line and
/// decodes uniquely, so distinct inputs never share a preimage.
fn preimage(inputs: &KeyInputs) -> Vec<u8> {
    let mut out = Vec::with_capacity(1024 + inputs.closure.len() * 128);
    out.extend_from_slice(FORMAT_LINE);
    let toolchain = &inputs.toolchain;
    line(
        &mut out,
        "toolchain.engine_revision",
        toolchain.engine_revision.as_bytes(),
    );
    line(
        &mut out,
        "toolchain.flutter_version",
        toolchain.flutter_version.as_bytes(),
    );
    line(
        &mut out,
        "toolchain.platform_strong_dill",
        blob_id_hex(&toolchain.platform_strong_dill).as_bytes(),
    );
    line(
        &mut out,
        "toolchain.frontend_server_snapshot",
        blob_id_hex(&toolchain.frontend_server_snapshot).as_bytes(),
    );
    let target = &inputs.target;
    line(&mut out, "target.command", target.command.as_bytes());
    line(&mut out, "target.platform", target.platform.as_bytes());
    line(&mut out, "target.mode", target.mode.as_bytes());
    if let Some(entry) = &target.entry {
        line(
            &mut out,
            "target.entry",
            entry.as_os_str().as_encoded_bytes(),
        );
    }
    if let Some(flavor) = &target.flavor {
        line(&mut out, "target.flavor", flavor.as_bytes());
    }
    for arg in &target.extra_key_args {
        line(&mut out, "target.extra_key_arg", arg.as_bytes());
    }
    for (name, value) in &inputs.defines {
        out.extend_from_slice(b"define ");
        escape(&mut out, name.as_bytes(), b"=");
        out.push(b'=');
        escape(&mut out, value.as_bytes(), b"");
        out.push(b'\n');
    }
    let mut closure: Vec<&ClosureFile> = inputs.closure.iter().collect();
    closure.sort_unstable_by(|a, b| closure_order(a, b));
    for file in closure {
        let (tag, path) = order_key(&file.identity);
        out.extend_from_slice(&[tag, b':']);
        escape(&mut out, path, b"");
        out.push(b' ');
        out.extend_from_slice(blob_id_hex(&file.content).as_bytes());
        out.push(b' ');
        out.extend_from_slice(file.size.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

/// Appends a `<label> <value>` line.
fn line(out: &mut Vec<u8>, label: &str, value: &[u8]) {
    out.extend_from_slice(label.as_bytes());
    out.push(b' ');
    escape(out, value, b"");
    out.push(b'\n');
}

/// Appends `value`, backslash-escaping `\`, LF, CR, and every byte of
/// `extra`.
fn escape(out: &mut Vec<u8>, value: &[u8], extra: &[u8]) {
    for &byte in value {
        match byte {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ if extra.contains(&byte) => out.extend_from_slice(&[b'\\', byte]),
            _ => out.push(byte),
        }
    }
}

/// A closure file's order key, as its tag and path bytes: `T:<rel>` or
/// `A:<abs>`. Paths contribute their raw OS bytes, which are UTF-8 for
/// UTF-8 paths on every platform.
fn order_key(identity: &FileIdentity) -> (u8, &[u8]) {
    match identity {
        FileIdentity::TreeRelative(relative) => (b'T', relative.as_os_str().as_encoded_bytes()),
        FileIdentity::Absolute(path) => (b'A', path.as_os_str().as_encoded_bytes()),
    }
}

/// Total order on closure files: order key, then content, then size.
fn closure_order(a: &ClosureFile, b: &ClosureFile) -> Ordering {
    order_key(&a.identity)
        .cmp(&order_key(&b.identity))
        .then_with(|| a.content.cmp(&b.content))
        .then_with(|| a.size.cmp(&b.size))
}

/// Streams `path` through blake3 with `buf`, returning the digest and the
/// number of bytes hashed: the size that matches the digest even if the
/// file changes meanwhile.
fn hash_file(path: &Path, buf: &mut [u8]) -> io::Result<(BlobId, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    loop {
        match file.read(buf) {
            Ok(0) => return Ok((BlobId(*hasher.finalize().as_bytes()), hasher.count())),
            Ok(n) => {
                hasher.update(&buf[..n]);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

/// Streamed blake3 of a toolchain artifact.
fn hash_artifact(path: &Path) -> Result<BlobId> {
    hash_file(path, &mut vec![0u8; HASH_BUF])
        .map(|(id, _)| id)
        .map_err(|err| io_error(path, err))
}

/// `frameworkVersion` of a `flutter.version.json` manifest.
fn framework_version(manifest: &Path) -> Result<String> {
    #[derive(Deserialize)]
    struct VersionManifest {
        #[serde(rename = "frameworkVersion")]
        framework_version: String,
    }

    let bytes = fs::read(manifest).map_err(|err| io_error(manifest, err))?;
    serde_json::from_slice::<VersionManifest>(&bytes)
        .map(|parsed| parsed.framework_version)
        .map_err(|err| BcacheError::Invalid(format!("{}: {err}", manifest.display())))
}

fn read_trimmed(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path).map_err(|err| io_error(path, err))?;
    Ok(text.trim().to_owned())
}

/// Names `path` in an I/O failure: a missing path is
/// [`BcacheError::NotFound`]; any other failure stays
/// [`BcacheError::Io`], kind preserved, with the path in its message.
fn io_error(path: &Path, err: io::Error) -> BcacheError {
    if err.kind() == io::ErrorKind::NotFound {
        BcacheError::NotFound(path.display().to_string())
    } else {
        BcacheError::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TargetSpec;

    /// The canonical preimage of [`sample_inputs`], spelled out by hand.
    const GOLDEN_PREIMAGE: &str = "\
format: bcache-key-v3
toolchain.engine_revision 5d531788691ec3404cac0cee66ead4007b177363
toolchain.flutter_version 3.47.1
toolchain.platform_strong_dill 1111111111111111111111111111111111111111111111111111111111111111
toolchain.frontend_server_snapshot 2222222222222222222222222222222222222222222222222222222222222222
target.command build
target.platform android
target.mode release
target.entry lib/main.dart
target.extra_key_arg --obfuscate
target.extra_key_arg --split-debug-info=build/symbols
define API_URL=https://example.com/v1?a=b
define FLAG=
A:/opt/flutter/bin/cache/artifacts/material_fonts/MaterialIcons-Regular.otf cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc 7
T:lib/main.dart aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1234
T:packages/characters/lib/src/characters.dart bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 56
";

    /// A tempdir and its canonical path (macOS tempdirs sit behind the
    /// `/var` -> `/private/var` symlink, and closure roots are canonical).
    fn tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        (dir, root)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture dirs");
        fs::write(path, bytes).expect("write fixture");
    }

    fn blob(byte: u8) -> BlobId {
        BlobId([byte; 32])
    }

    fn digest(bytes: &[u8]) -> BlobId {
        BlobId(*blake3::hash(bytes).as_bytes())
    }

    fn defines(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|&(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }

    fn sample_inputs() -> KeyInputs {
        KeyInputs {
            toolchain: ToolchainFingerprint {
                engine_revision: "5d531788691ec3404cac0cee66ead4007b177363".to_owned(),
                flutter_version: "3.47.1".to_owned(),
                platform_strong_dill: blob(0x11),
                frontend_server_snapshot: blob(0x22),
            },
            defines: defines(&[("API_URL", "https://example.com/v1?a=b"), ("FLAG", "")]),
            closure: vec![
                ClosureFile {
                    identity: FileIdentity::TreeRelative("lib/main.dart".into()),
                    content: blob(0xaa),
                    size: 1234,
                },
                ClosureFile {
                    identity: FileIdentity::TreeRelative(
                        "packages/characters/lib/src/characters.dart".into(),
                    ),
                    content: blob(0xbb),
                    size: 56,
                },
                ClosureFile {
                    identity: FileIdentity::Absolute(
                        "/opt/flutter/bin/cache/artifacts/material_fonts/MaterialIcons-Regular.otf"
                            .into(),
                    ),
                    content: blob(0xcc),
                    size: 7,
                },
            ],
            target: TargetSpec {
                command: "build".to_owned(),
                platform: "android".to_owned(),
                mode: "release".to_owned(),
                entry: Some("lib/main.dart".into()),
                flavor: None,
                extra_key_args: vec![
                    "--obfuscate".to_owned(),
                    "--split-debug-info=build/symbols".to_owned(),
                ],
            },
        }
    }

    fn key_with(edit: impl FnOnce(&mut KeyInputs)) -> String {
        let mut inputs = sample_inputs();
        edit(&mut inputs);
        config_key(&inputs)
    }

    #[cfg(unix)]
    #[test]
    fn flutter_root_follows_symlinked_shim() {
        let (_guard, tmp) = tempdir();
        let sdk = tmp.join("versions").join("3.47.1");
        write(&sdk.join("bin").join("flutter"), b"#!/bin/sh\n");
        // fvm layout: a symlinked SDK directory, reached through a
        // symlinked binary.
        std::os::unix::fs::symlink(&sdk, tmp.join("default")).expect("symlink sdk");
        fs::create_dir_all(tmp.join("shims")).expect("create shims dir");
        let shim = tmp.join("shims").join("flutter");
        std::os::unix::fs::symlink(tmp.join("default").join("bin").join("flutter"), &shim)
            .expect("symlink shim");
        assert_eq!(flutter_root(&shim).expect("flutter_root"), sdk);
    }

    #[test]
    fn fingerprint_toolchain_reads_the_pinned_layout() {
        let (_guard, root) = tempdir();
        let cache = root.join("bin").join("cache");
        write(
            &cache.join("engine-dart-sdk.stamp"),
            b"5d531788691ec3404cac0cee66ead4007b177363\n",
        );
        write(
            &cache.join("flutter.version.json"),
            br#"{"frameworkVersion": "3.47.1", "channel": "stable"}"#,
        );
        let dill = b"platform dill bytes";
        let aot = b"aot frontend_server bytes";
        write(
            &cache.join("dart-sdk/lib/_internal/vm_platform_strong.dill"),
            dill,
        );
        write(
            &cache.join("dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot"),
            aot,
        );
        assert_eq!(
            fingerprint_toolchain(&root).expect("fingerprint"),
            ToolchainFingerprint {
                engine_revision: "5d531788691ec3404cac0cee66ead4007b177363".to_owned(),
                flutter_version: "3.47.1".to_owned(),
                platform_strong_dill: digest(dill),
                frontend_server_snapshot: digest(aot),
            }
        );
    }

    #[test]
    fn fingerprint_toolchain_reports_missing_artifacts() {
        let (_guard, root) = tempdir();
        let cache = root.join("bin").join("cache");
        write(
            &cache.join("dart-sdk/lib/_internal/vm_platform_strong.dill"),
            b"dill",
        );
        // No engine stamp at all.
        let err = fingerprint_toolchain(&root).expect_err("the stamp is missing");
        assert!(
            matches!(&err, BcacheError::NotFound(path) if path.ends_with("engine-dart-sdk.stamp")),
            "got {err:?}"
        );

        write(&cache.join("engine-dart-sdk.stamp"), b"rev\n");
        write(
            &cache.join("flutter.version.json"),
            br#"{"frameworkVersion": "3.47.1"}"#,
        );
        // The dill exists, the aot snapshot does not.
        let err = fingerprint_toolchain(&root).expect_err("the aot snapshot is missing");
        assert!(
            matches!(&err, BcacheError::NotFound(path) if path
                .ends_with("frontend_server_aot.dart.snapshot")),
            "got {err:?}"
        );
    }

    #[test]
    fn config_key_is_order_insensitive() {
        let inputs = sample_inputs();
        let key = config_key(&inputs);
        let mut rotated = inputs.clone();
        rotated.closure.rotate_left(1);
        let mut reversed = inputs.clone();
        reversed.closure.reverse();
        assert_eq!(config_key(&rotated), key);
        assert_eq!(config_key(&reversed), key);
    }

    #[test]
    fn config_key_tracks_content_and_defines() {
        let key = config_key(&sample_inputs());
        assert_ne!(key_with(|inputs| inputs.closure[1].content.0[31] ^= 1), key);
        assert_ne!(
            key_with(|inputs| {
                inputs.defines.insert("EXTRA".to_owned(), "1".to_owned());
            }),
            key
        );
    }

    #[test]
    fn config_key_keeps_field_boundaries() {
        let one_define =
            |name: &str, value: &str| key_with(|inputs| inputs.defines = defines(&[(name, value)]));
        assert_ne!(one_define("a=b", "c"), one_define("a", "b=c"));
        assert_ne!(
            key_with(|inputs| inputs.target.flavor = None),
            key_with(|inputs| inputs.target.flavor = Some(String::new()))
        );
        // An embedded newline cannot forge a second line.
        assert_ne!(
            key_with(|inputs| {
                inputs.target.extra_key_args = vec!["--a\ntarget.extra_key_arg --b".to_owned()];
            }),
            key_with(|inputs| {
                inputs.target.extra_key_args = vec!["--a".to_owned(), "--b".to_owned()];
            })
        );
    }

    #[test]
    fn config_key_matches_golden_vector() {
        let inputs = sample_inputs();
        let preimage = String::from_utf8(preimage(&inputs)).expect("utf-8 preimage");
        assert_eq!(preimage, GOLDEN_PREIMAGE);
        // The key is the blake3 of the hand-written preimage above, in
        // canonical 64-hex spelling.
        let key = config_key(&inputs);
        assert_eq!(
            key,
            blake3::hash(GOLDEN_PREIMAGE.as_bytes()).to_hex().to_string()
        );
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn config_key_hashes_provenance_and_coarse_inputs() {
        let inputs = sample_inputs();
        // Provenance contents are in the key: an edited provenance file
        // is a different configuration.
        assert_ne!(
            config_key(&inputs),
            {
                let mut edited = sample_inputs();
                edited.closure[0].content = blob(0xad);
                config_key(&edited)
            }
        );
        // A dropped provenance file is a different configuration too.
        assert_ne!(
            config_key(&inputs),
            {
                let mut edited = sample_inputs();
                edited.closure.pop();
                config_key(&edited)
            }
        );
        // ...as is any other coarse-key input.
        assert_ne!(
            config_key(&sample_inputs()),
            {
                let mut edited = sample_inputs();
                edited.defines.insert("EXTRA".to_owned(), "1".to_owned());
                config_key(&edited)
            }
        );
    }

    #[test]
    fn filter_defines_drops_only_flutter_git_url() {
        let raw = defines(&[
            ("API_URL", "https://example.com"),
            ("FLUTTER_GIT_URL", "https://github.com/flutter/flutter.git"),
            ("FLUTTER_VERSION", "3.47.1"),
        ]);
        assert_eq!(
            filter_defines(&raw),
            defines(&[
                ("API_URL", "https://example.com"),
                ("FLUTTER_VERSION", "3.47.1")
            ])
        );
    }
}
