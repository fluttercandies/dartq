//! The conformance pin (`conformance/pin.toml`): the single source of
//! truth naming the exact Dart SDK every generated registry mirrors.
//!
//! Tracked code never references a local SDK path; the checkout root is
//! given per-run (`--sdk-src` or `DARTQ_SDK_SRC`) and must be the tree of
//! exactly the pinned version on the stable channel. Generated documents
//! stamp the pinned revision in an `SDK pin:` header line, which `--check`
//! verifies against the pin before the byte comparison.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The parsed pin file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// `sdk_version`, e.g. `3.13.7`.
    pub sdk_version: String,
    /// `sdk_revision`: the 40-hex git revision of the SDK checkout.
    pub sdk_revision: String,
}

impl Pin {
    /// Loads `<workspace-root>/conformance/pin.toml` (the workspace root
    /// is two levels above this crate's manifest dir).
    pub fn load() -> Result<Pin, String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/pin.toml");
        let text =
            fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}:{e}", path.display()))
    }

    /// Parses the pin file: exactly the two known keys, `key = "value"`
    /// lines; comments and blank lines allowed, anything else rejected so
    /// a schema change cannot be silently ignored.
    fn parse(src: &str) -> Result<Pin, String> {
        let mut sdk_version = None;
        let mut sdk_revision = None;
        for (no, line) in src.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |what: &str| format!("{no}: {what}");
            let Some((key, value)) = line.split_once('=') else {
                return Err(err("not a `key = \"value\"` line"));
            };
            let Some(value) = value
                .trim()
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
            else {
                return Err(err("value is not a double-quoted string"));
            };
            match key.trim() {
                "sdk_version" => sdk_version = Some(value.to_owned()),
                "sdk_revision" => sdk_revision = Some(value.to_owned()),
                other => return Err(err(&format!("unknown key `{other}`"))),
            }
        }
        let sdk_version = sdk_version.ok_or("missing `sdk_version`")?;
        let sdk_revision = sdk_revision.ok_or("missing `sdk_revision`")?;
        if sdk_revision.len() != 40 || !sdk_revision.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "`sdk_revision` `{sdk_revision}` is not a 40-hex git revision"
            ));
        }
        Ok(Pin {
            sdk_version,
            sdk_revision,
        })
    }

    /// Verifies the checkout at `sdk_src` is the pinned SDK: `tools/VERSION`
    /// (the file the SDK's own version tooling generates from) must spell
    /// the pinned MAJOR.MINOR.PATCH on the stable channel. Returns the
    /// header stamp, e.g. `dart 3.13.7, stable`.
    pub fn verify_checkout(&self, sdk_src: &Path) -> Result<String, String> {
        let version_path = sdk_src.join("tools/VERSION");
        let text = fs::read_to_string(&version_path).map_err(|e| {
            format!(
                "reading {}: {e} (is this a Dart SDK tree?)",
                version_path.display()
            )
        })?;
        let mut fields = BTreeMap::new();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once(' ') {
                fields.insert(key.to_owned(), value.trim().to_owned());
            }
        }
        let get = |key: &str| fields.get(key).cloned().unwrap_or_default();
        let channel = get("CHANNEL");
        let version = format!("{}.{}.{}", get("MAJOR"), get("MINOR"), get("PATCH"));
        if version != self.sdk_version || channel != "stable" {
            return Err(format!(
                "{}: checkout is {version} ({channel}); the pin requires {} (stable)",
                version_path.display(),
                self.sdk_version
            ));
        }
        Ok(format!("dart {version}, {channel}"))
    }

    /// The header stamp line naming the revision; renderers prepend their
    /// own comment prefix: `SDK pin: <revision> (dart 3.13.7, stable)`.
    pub fn stamp(&self, checkout_stamp: &str) -> String {
        format!("SDK pin: {} ({})", self.sdk_revision, checkout_stamp)
    }
}

/// The revision a generated document was produced from, taken from its
/// `SDK pin: <rev>` header line; `None` when the line is absent (a
/// hand-edited or pre-pin document).
pub fn header_revision(document: &str) -> Option<&str> {
    document.lines().find_map(|line| {
        let at = line.find("SDK pin: ")?;
        line[at + "SDK pin: ".len()..].split(' ').next()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_pin_file() {
        let pin = Pin::parse(
            "# comment\nsdk_version = \"3.13.7\"\nsdk_revision = \
             \"416e33b0ffaef062a60ec966bb8c82ed2b83f128\"\n",
        )
        .expect("parses");
        assert_eq!(pin.sdk_version, "3.13.7");
        assert_eq!(pin.sdk_revision, "416e33b0ffaef062a60ec966bb8c82ed2b83f128");
    }

    #[test]
    fn rejects_malformed_pins() {
        assert!(Pin::parse("sdk_version = \"3.13.7\"\n").is_err());
        assert!(
            Pin::parse("sdk_revision = \"416e33b0ffaef062a60ec966bb8c82ed2b83f128\"\n").is_err()
        );
        assert!(Pin::parse("sdk_version = \"3.13.7\"\nsdk_revision = \"abc\"\n").is_err());
        assert!(Pin::parse("sdk_version = 3.13.7\n").is_err());
        assert!(Pin::parse("surprise = \"x\"\n").is_err());
    }

    #[test]
    fn header_revision_extracts_the_stamp() {
        let doc =
            "# header\n# SDK pin: 416e33b0ffaef062a60ec966bb8c82ed2b83f128 (dart 3.13.7, stable)\n";
        assert_eq!(
            header_revision(doc),
            Some("416e33b0ffaef062a60ec966bb8c82ed2b83f128")
        );
        assert_eq!(header_revision("# nothing\n"), None);
    }
}
