//! Locating the concrete Dart SDK root behind an SDK-ish path.
//!
//! Two layouts ship a Dart SDK: the plain install (`<root>/bin/dart`,
//! `<root>/lib/libraries.json`) and every Flutter checkout, whose Dart SDK
//! sits embedded at `<root>/bin/cache/dart-sdk` — the layout the flutter
//! tool itself resolves `dart` through and the analyzer's official face
//! resolves a `dart` binary through. [`dart_sdk_root`] accepts either
//! spelling of a root; [`dart_sdk_root_of_dart`] accepts either spelling of
//! a `dart` executable.

use std::fs;
use std::path::{Path, PathBuf};

/// The concrete Dart SDK root `root` names: the root itself when it holds
/// a plain SDK, else the Dart SDK a Flutter root embeds at
/// `bin/cache/dart-sdk` — canonicalized (realpath), so an fvm `default`
/// symlink and the version directory it points at resolve to one path and
/// host aliases (macOS `/var` → `/private/var`) never leak into the
/// engine's realpath-normalized file table.
///
/// The probe is [`SdkSnapshot::load`](crate::SdkSnapshot::load)'s own
/// face: a root qualifies exactly when it holds `lib/libraries.json`, so
/// whatever this accepts, the loader can read — a resolver built on it can
/// never hand the loader a root it would refuse. `None` when neither
/// layout holds: the caller's usage tier, never a silent guess.
#[must_use]
pub fn dart_sdk_root(root: &Path) -> Option<PathBuf> {
    let candidate = sdk_layout(root).or_else(|| flutter_embedded(root))?;
    Some(fs::canonicalize(&candidate).unwrap_or(candidate))
}

/// The concrete Dart SDK root behind a `dart` executable: the plain layout
/// (`<sdk>/bin/dart`) or Flutter's wrapper script (`<flutter>/bin/dart`,
/// SDK at `<flutter>/bin/cache/dart-sdk`). `None` when the binary sits in
/// neither layout.
#[must_use]
pub fn dart_sdk_root_of_dart(dart: &Path) -> Option<PathBuf> {
    let bin = dart.parent()?;
    dart_sdk_root(&bin.join("cache").join("dart-sdk")).or_else(|| {
        let root = bin.parent()?;
        dart_sdk_root(root)
    })
}

/// `root` itself is a plain Dart SDK root.
fn sdk_layout(root: &Path) -> Option<PathBuf> {
    root.join("lib")
        .join("libraries.json")
        .is_file()
        .then(|| root.to_path_buf())
}

/// `root` is a Flutter SDK root; the Dart SDK it embeds.
fn flutter_embedded(root: &Path) -> Option<PathBuf> {
    let embedded = root.join("bin").join("cache").join("dart-sdk");
    embedded
        .join("lib")
        .join("libraries.json")
        .is_file()
        .then_some(embedded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory under the system temp root, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("dartq-sdk-locate-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir.canonicalize().expect("scratch canonicalizes"))
        }

        fn write(&self, relative: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(&path, "").expect("file");
        }

        /// A minimal plain Dart SDK at `<tag>/dart-sdk`.
        fn dart_sdk(&self) -> PathBuf {
            self.0.join("dart-sdk")
        }

        /// A minimal Flutter root at `<tag>/flutter` embedding the SDK.
        fn flutter_root(&self) -> PathBuf {
            self.0.join("flutter")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A plain SDK root names itself; a Flutter root names its embedded
    /// `bin/cache/dart-sdk`; anything else names nothing.
    #[test]
    fn both_sdk_layouts_resolve_and_noise_does_not() {
        let scratch = Scratch::new("layouts");
        let sdk = scratch.dart_sdk();
        scratch.write("dart-sdk/version");
        scratch.write("dart-sdk/lib/libraries.json");
        assert_eq!(dart_sdk_root(&sdk), Some(sdk.clone()));

        let flutter = scratch.flutter_root();
        scratch.write("flutter/bin/dart");
        scratch.write("flutter/bin/cache/dart-sdk/version");
        scratch.write("flutter/bin/cache/dart-sdk/lib/libraries.json");
        assert_eq!(
            dart_sdk_root(&flutter),
            Some(flutter.join("bin").join("cache").join("dart-sdk"))
        );

        assert_eq!(dart_sdk_root(&scratch.0.join("nothing")), None);
        // A directory with files but no SDK metadata is not an SDK root.
        scratch.write("noise/readme.txt");
        assert_eq!(dart_sdk_root(&scratch.0.join("noise")), None);
    }

    /// A symlinked root canonicalizes: the link and its target name one
    /// path, and `dart_sdk_root` never answers a link-spelled SDK root.
    #[cfg(unix)]
    #[test]
    fn symlinked_roots_canonicalize() {
        let scratch = Scratch::new("symlink");
        let sdk = scratch.dart_sdk();
        scratch.write("dart-sdk/version");
        scratch.write("dart-sdk/lib/libraries.json");
        let link = scratch.0.join("linked-sdk");
        std::os::unix::fs::symlink(&sdk, &link).expect("symlink");
        assert_eq!(dart_sdk_root(&link), Some(sdk));
    }

    /// Both `dart` layouts resolve to the SDK root behind the binary; a
    /// free-standing binary resolves to nothing.
    #[test]
    fn dart_binaries_resolve_through_both_layouts() {
        let scratch = Scratch::new("binaries");
        let sdk = scratch.dart_sdk();
        scratch.write("dart-sdk/bin/dart");
        scratch.write("dart-sdk/version");
        scratch.write("dart-sdk/lib/libraries.json");
        assert_eq!(
            dart_sdk_root_of_dart(&sdk.join("bin").join("dart")),
            Some(sdk)
        );

        let flutter = scratch.flutter_root();
        scratch.write("flutter/bin/dart");
        scratch.write("flutter/bin/cache/dart-sdk/version");
        scratch.write("flutter/bin/cache/dart-sdk/lib/libraries.json");
        assert_eq!(
            dart_sdk_root_of_dart(&flutter.join("bin").join("dart")),
            Some(flutter.join("bin").join("cache").join("dart-sdk"))
        );

        scratch.write("loose/bin/dart");
        assert_eq!(
            dart_sdk_root_of_dart(&scratch.0.join("loose/bin/dart")),
            None
        );
    }
}
