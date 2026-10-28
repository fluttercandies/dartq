//! The pubspec family: `secure_pubspec_urls` (core), `package_names`
//! (recommended), `depend_on_referenced_packages` (core), and
//! `sort_pub_dependencies` (standalone) — the official home is
//! `pkg/linter/lib/src/rules/pub/` — plus the analyzer pubspec
//! dependency validator's `path:` family (`path_not_posix`,
//! `path_does_not_exist`, `path_pubspec_does_not_exist`,
//! upstream Dart SDK source,
//! `pkg/analyzer/lib/src/pubspec/validators/dependency_validator.dart`).
//!
//! Two mounting shapes. `secure_pubspec_urls` and `package_names` read
//! `pubspec.yaml` itself, which the engine's [`Unit`][crate::Unit] does not
//! carry: the body is a pure function over one pubspec fact (one URL value
//! / one `name:` value, with its byte range) returning report rows; the
//! [`RuleSpec`] sits in [`crate::RULES`] for name/group/count alignment
//! with `kinds: &[]`, and the pubspec pass at the mounting face
//! (`dartq-commands` `analyze_lints`) walks the visited fields (the
//! official visitor's :24-58) over the scanned facts
//! (`dartq_pub::pubspec_facts`). `depend_on_referenced_packages` rides the
//! normal AST dispatch (`kinds: [Import, Export]`) off the
//! [`WorkspaceFacts`][crate::WorkspaceFacts] the mount attaches per unit.

use dartq_ast::NodeKind;

use crate::registry::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

pub(crate) mod path_dependencies {
    //! Official: the analyzer pubspec dependency validator's `path:` checks
    //! (`validatePathEntries`, upstream Dart SDK source,
    //! `pkg/analyzer/lib/src/pubspec/validators/dependency_validator.dart`
    //! :42-93) — every entry of `dependencies` and `dev_dependencies`
    //! (:113-115, :117-128; `dependency_overrides` is never read) whose
    //! value is a map carrying a string-valued `path:` field
    //! (`PubspecField.PATH_FIELD`, `asString` :155-162) is validated in
    //! order (:51-77): a value containing `\` reports `path_not_posix`
    //! and returns; otherwise the resolved folder
    //! (`posix.split` → join under the pubspec's directory →
    //! absolute → normalize, :58-63) that does not exist reports
    //! `path_does_not_exist`, and one that exists without a
    //! `pubspec.yaml` inside reports `path_pubspec_does_not_exist`.
    //! All three report at the `path` VALUE node (`YamlScalar.span`, the
    //! raw scalar extent, quotes included) and are `STATIC_WARNING`s.
    //! Message templates: `diagnostic.g.dart` `pathNotPosix`
    //! (:14003-14011), `pathDoesNotExist` (:13986-13995),
    //! `pathPubspecDoesNotExist` (:14019-14029). The validator's
    //! `checkForPathAndGitDeps` arm (:78-83) emits the separate
    //! `invalid_dependency` code at the path KEY — not this family.
    //!
    //! [`Group::Diagnostic`] — analyzer diagnostics outside
    //! `package:lints`, the `unnecessary_type_check` mirror posture:
    //! registered so the names resolve and the templates render
    //! oracle-exact; `kinds: &[]` — the verdict runs per pubspec entry,
    //! not per AST node, and the pubspec pass at the mounting face
    //! (dartq-commands' `analyze_lints`, the same face the other pubspec
    //! rules ride) feeds [`scan_path_entries`] facts through
    //! [`check_path_entry`], resolving existence against the folder
    //! [`resolve_dependency_path`] names (the mounter's I/O; a path
    //! resolving to a regular file reads as a missing folder — the
    //! official `getFolder(..).exists` face).
    //!
    //! Recognition faces (the scan is a targeted line walk, the
    //! `dartq_pub::pubspec_facts` tolerance posture): block-form dep
    //! entries only — flow maps (`dep: {path: x}`) and quoted keys are
    //! under-matched (no row where the official reports), and block
    //! scalars (`dep: |` bodies) can over-match (a `path:` line inside a
    //! scalar body records where the official sees a string value, not a
    //! map). Anchors/aliases are under-matched. A plain scalar that YAML
    //! types as null/bool/number is not a String (`asString` null — the
    //! official stays silent), so the scan applies the `yaml` package's
    //! scalar resolution (`loader.dart` `_tryParseScalar` :239-259,
    //! `_parseNumberValue` :281-379) and records plain scalars only when
    //! they stay strings.

    use std::path::{Path, PathBuf};

    use super::{Group, RuleSpec, not_dispatched};

    /// `path_not_posix` (`diagnostic.g.dart` :14003-14011).
    pub(crate) static PATH_NOT_POSIX: RuleSpec = RuleSpec {
        name: "path_not_posix",
        group: Group::Diagnostic,
        problem: "The path '{0}' isn't a POSIX-style path.",
        correction: "Try converting the value to a POSIX-style path.",
        kinds: &[],
        check: not_dispatched,
    };

    /// `path_does_not_exist` (`diagnostic.g.dart` :13986-13995).
    pub(crate) static PATH_DOES_NOT_EXIST: RuleSpec = RuleSpec {
        name: "path_does_not_exist",
        group: Group::Diagnostic,
        problem: "The path '{0}' doesn't exist.",
        correction: "Try creating the referenced path or using a path that exists.",
        kinds: &[],
        check: not_dispatched,
    };

    /// `path_pubspec_does_not_exist` (`diagnostic.g.dart` :14019-14029) —
    /// the family's headline code; the three specs share the one upstream
    /// validator.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "path_pubspec_does_not_exist",
        group: Group::Diagnostic,
        problem: "The directory '{0}' doesn't contain a pubspec.",
        correction: "Try creating a pubspec in the referenced directory or using a path \
             that has a pubspec.",
        kinds: &[],
        check: not_dispatched,
    };

    /// One `path:` fact: `range` is the raw value extent in the scanned
    /// text (quotes included — the `YamlNode.span` face, the report
    /// anchor) and `path` the cooked scalar text (the messages' `{0}` and
    /// the resolution input).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PathEntry {
        pub range: core::ops::Range<usize>,
        pub path: String,
    }

    /// The `validatePathEntries` verdict over one `path:` value, in the
    /// official order (:51-77): the POSIX-shape gate first, then folder
    /// existence, then the folder's own pubspec.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PathVerdict {
        NotPosix,
        DoesNotExist,
        PubspecDoesNotExist,
    }

    /// The official `validatePathEntries` body (:51-77) over one path
    /// value with the mounter's existence facts: `folder_exists` reads
    /// `getFolder(dependencyPath).exists` (a regular file at the resolved
    /// path is a missing folder) and `pubspec_exists` reads
    /// `getChild(pubspecYaml).exists`. `Some(verdict)` reports at the
    /// value node.
    #[must_use]
    pub fn check_path_entry(
        path: &str,
        folder_exists: bool,
        pubspec_exists: bool,
    ) -> Option<PathVerdict> {
        if path.contains('\\') {
            return Some(PathVerdict::NotPosix);
        }
        if !folder_exists {
            return Some(PathVerdict::DoesNotExist);
        }
        if !pubspec_exists {
            return Some(PathVerdict::PubspecDoesNotExist);
        }
        None
    }

    /// The official resolution (:58-63) over the posix host: `path` is
    /// split on `/` (`path.posix.split` — empty parts dropped), joined
    /// under `pubspec_dir` (`context.join` — a rooted `path` replaces the
    /// base), then normalized (`context.normalize`, parsed_path.dart
    /// :100-129 — `.` and empty parts drop, `..` pops, a relative path
    /// keeps the `..` it backs out past, an absolute one clamps at the
    /// root, and a relative result collapsed to nothing reads `.`). The
    /// folder the mounter stats.
    #[must_use]
    pub fn resolve_dependency_path(pubspec_dir: &Path, path: &str) -> PathBuf {
        let rooted = path.starts_with('/') || pubspec_dir.is_absolute();
        let base: Vec<&str> = if path.starts_with('/') {
            Vec::new()
        } else {
            pubspec_dir
                .components()
                .filter_map(|component| match component {
                    std::path::Component::Normal(part) => Some(part.to_str().unwrap_or_default()),
                    std::path::Component::ParentDir => Some(".."),
                    _ => None,
                })
                .collect()
        };
        let mut parts: Vec<&str> = Vec::with_capacity(base.len());
        let mut leading = 0usize;
        for part in base.into_iter().chain(path.split('/')) {
            match part {
                "" | "." => {}
                ".." if parts.is_empty() => leading += 1,
                ".." => {
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        let mut out = PathBuf::new();
        if rooted {
            out.push("/");
        } else {
            for _ in 0..leading {
                out.push("..");
            }
            if leading == 0 && parts.is_empty() {
                out.push(".");
            }
        }
        for part in parts {
            out.push(part);
        }
        out
    }

    /// Scans the `path:` entries the validator judges, in document order:
    /// a top-level `dependencies:`/`dev_dependencies:` block (`:113-128`;
    /// `dependency_overrides` is never read), each block-form dep entry's
    /// first spec level (`dependency[PATH_FIELD]` — a `path:` nested
    /// deeper, e.g. under `git:`, is a different field), and only
    /// string-valued scalars (`asString` :155-162: quoted scalars are
    /// strings; a plain scalar survives the `yaml` package's
    /// null/bool/number resolution or it is not a String). Linear in the
    /// text; the recognition faces are the module doc's.
    #[must_use]
    pub fn scan_path_entries(text: &str) -> Vec<PathEntry> {
        let mut scan = PathScan::default();
        let mut offset = 0usize;
        for raw in text.split_inclusive('\n') {
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            scan.line(line, offset);
            offset += raw.len();
        }
        scan.entries
    }

    /// The per-line walk state: the open dependency section, its dep-name
    /// indent, and the spec level the section's current dep fixes.
    #[derive(Default)]
    struct PathScan {
        entries: Vec<PathEntry>,
        section: bool,
        dep_indent: Option<usize>,
        spec_indent: Option<usize>,
    }

    impl PathScan {
        fn line(&mut self, line: &str, offset: usize) {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let body = &line[indent..];
            // Blank and comment-only lines carry no keys.
            if body.is_empty() || body.starts_with('#') {
                return;
            }
            let Some(colon) = find_key_colon(body) else {
                return;
            };
            let key = body[..colon].trim();
            let (range, value, quoted) = scalar(&body[colon + 1..], offset + indent + colon + 1);

            if indent == 0 {
                // A dependency section opens only in block form; any other
                // top-level key closes the current one.
                self.section =
                    matches!(key, "dependencies" | "dev_dependencies") && value.is_empty();
                self.dep_indent = None;
                self.spec_indent = None;
                return;
            }
            if !self.section {
                return;
            }
            match self.dep_indent {
                None => {
                    // The first indented line fixes the dep-name indent;
                    // the dep line itself opens the entry and is never
                    // the spec's `path:` field.
                    self.dep_indent = Some(indent);
                    self.spec_indent = None;
                    return;
                }
                Some(dep_indent) if indent <= dep_indent => {
                    // The next dep entry (or the section's end): the
                    // previous spec is over.
                    self.dep_indent = Some(indent);
                    self.spec_indent = None;
                    return;
                }
                Some(_) => {}
            }
            match self.spec_indent {
                None => self.spec_indent = Some(indent),
                Some(spec_indent) if indent != spec_indent => return,
                Some(_) => {}
            }
            // A spec-level `path:` with a string value is the validator's
            // field: an empty value is block form (a null — silent), a
            // quoted scalar is a String, and a plain scalar must survive
            // the yaml resolution to read as one.
            if key == "path" && !value.is_empty() && (quoted || yaml_plain_is_string(&value)) {
                self.entries.push(PathEntry { range, path: value });
            }
        }
    }

    /// Scans the `path:` scalars inside the top-level `screenshots:` list —
    /// the analyzer's `screenshotsValidator` walk (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/pubspec/validators/screenshot_validator.dart`
    /// :12-38): `screenshots` must be a `YamlList` whose entries are
    /// `YamlMap`s, each map's `path` field a `YamlScalar` whose `value` is a
    /// `String`. A missing file reports `path_does_not_exist` at the value
    /// node.
    ///
    /// Shape parity with [`scan_path_entries`]: a top-level `screenshots:`
    /// key opens the section (block form only — `screenshots: <scalar>` is
    /// not a `YamlList`), an indented `- ` opens each entry, and the item's
    /// `path:` sits either inline on the dash line (`- path: a.png`) or at
    /// the item's key-indent on a following line (`- description: ...`
    /// then `path: a.png` under it). A second `path:` inside the same item
    /// or a `path:` deeper than the item's key-indent is not the map's
    /// `path` field and does not report. Linear in the text.
    #[must_use]
    pub fn scan_screenshot_entries(text: &str) -> Vec<PathEntry> {
        let mut scan = ScreenshotScan::default();
        let mut offset = 0usize;
        for raw in text.split_inclusive('\n') {
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            scan.line(line, offset);
            offset += raw.len();
        }
        scan.entries
    }

    /// Per-line walk state for [`scan_screenshot_entries`]: the open
    /// `screenshots:` section flag, the list-item indent (the column of
    /// the `- ` marker's owning line), and the item's key-indent (the
    /// column the item's `path:` key sits at — either the char after the
    /// `- ` for an inline key, or the first non-dash line's indent).
    #[derive(Default)]
    struct ScreenshotScan {
        entries: Vec<PathEntry>,
        section: bool,
        /// The indent of the `screenshots:` list's own `- ` items — fixed
        /// by the first `- ` under the section. A `- ` at a different
        /// indent belongs to a nested list inside an item's map value,
        /// never to `screenshots:` itself.
        list_indent: Option<usize>,
        item_indent: Option<usize>,
        key_indent: Option<usize>,
        seen_path_in_item: bool,
    }

    impl ScreenshotScan {
        fn line(&mut self, line: &str, offset: usize) {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let body = &line[indent..];
            // Blank and comment-only lines carry no keys.
            if body.is_empty() || body.starts_with('#') {
                return;
            }
            // A `- ` dash at the list's own indent opens a new list item.
            // A `- ` at any *other* indent is a nested list inside the
            // current item's map value — its `path:` is a different field
            // and must not open a new screenshot item.
            let is_dash_line = self.section && body.starts_with("- ");
            let item_dash = is_dash_line
                && match self.list_indent {
                    // The first dash under the section fixes the list's
                    // own indent.
                    None => {
                        self.list_indent = Some(indent);
                        true
                    }
                    Some(list) => indent == list,
                };
            let (content, content_offset) = if item_dash {
                (&body[2..], offset + indent + 2)
            } else {
                (body, offset + indent)
            };
            let Some(colon) = find_key_colon(content) else {
                // A non-key line ends any in-flight item.
                return;
            };
            let key = content[..colon].trim();
            let (range, value, quoted) = scalar(&content[colon + 1..], content_offset + colon + 1);

            if indent == 0 {
                // A `screenshots:` section opens only in block form; any
                // other top-level key closes it.
                self.section = key == "screenshots" && value.is_empty();
                self.list_indent = None;
                self.item_indent = None;
                self.key_indent = None;
                self.seen_path_in_item = false;
                return;
            }
            if !self.section {
                return;
            }
            if item_dash {
                // New list item at the list's own indent: the `-` itself
                // is the item indent; the inline `key:` (if this line
                // carries one) sits at virtual indent `indent + 2`.
                self.item_indent = Some(indent);
                self.key_indent = Some(indent + 2);
                self.seen_path_in_item = false;
            } else if is_dash_line {
                // A `- ` deeper than the list indent: a nested list inside
                // the current item's map value. The line's content is not
                // the item's own `path:` field regardless of what follows
                // the dash — treat the line as opaque and move on.
                return;
            } else {
                // A non-dash line: it belongs to the open item only when it
                // sits *deeper* than the item's own indent. A line at or
                // above the item indent closes the item.
                match self.item_indent {
                    None => return,
                    Some(item) if indent <= item => {
                        self.item_indent = None;
                        self.key_indent = None;
                        self.seen_path_in_item = false;
                        return;
                    }
                    Some(_) => {}
                }
                // Only lines at the item's key-indent are the item's own
                // fields; a deeper line is inside a nested map value.
                match self.key_indent {
                    Some(k) if indent != k => return,
                    Some(_) => {}
                    None => self.key_indent = Some(indent),
                }
            }
            if key == "path"
                && !self.seen_path_in_item
                && !value.is_empty()
                && (quoted || yaml_plain_is_string(&value))
            {
                self.entries.push(PathEntry { range, path: value });
                self.seen_path_in_item = true;
            }
        }
    }

    /// The first `:` of `body` that terminates a key (followed by a space
    /// or end of line — the `../a:b` colons inside path values never are).
    fn find_key_colon(body: &str) -> Option<usize> {
        let b = body.as_bytes();
        (0..b.len()).find(|&i| b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' '))
    }

    /// The inline scalar after `key:` (untrimmed, `abs` the offset just
    /// past the colon): its absolute byte range — quotes included — its
    /// cooked value, and whether it was quoted (a quoted scalar is a
    /// String whatever its text). An empty value is block form; a `#` at
    /// the value head or after whitespace ends a plain scalar; a
    /// double-quote escape outside the `\"`/`\\` pair fails the scalar
    /// closed (an empty value — no row).
    fn scalar(rest: &str, abs: usize) -> (core::ops::Range<usize>, String, bool) {
        let trimmed = rest.trim_start();
        let abs = abs + (rest.len() - trimmed.len());
        let b = trimmed.as_bytes();
        match b.first() {
            None | Some(b'#') => (abs..abs, String::new(), false),
            Some(b'\'') => quoted_scalar(trimmed, abs, b'\''),
            Some(b'"') => quoted_scalar(trimmed, abs, b'"'),
            _ => {
                // A `#` after any whitespace ends a plain scalar (`a#b`
                // has no comment). The plain arm never sees a leading `#`,
                // so every match has a preceding byte.
                let end = trimmed
                    .match_indices('#')
                    .find(|&(i, _)| matches!(trimmed.as_bytes()[i - 1], b' ' | b'\t'))
                    .map_or(trimmed.len(), |(i, _)| i);
                let value = trimmed[..end].trim_end();
                (abs..abs + value.len(), value.to_owned(), false)
            }
        }
    }

    /// A quoted scalar: the raw extent spans the quotes; the cooked value
    /// is the content with YAML's quote escapes resolved (single: `''`
    /// doubling; double: only `\"` and `\\` — any other escape fails the
    /// scalar closed). An unterminated literal is block form.
    fn quoted_scalar(
        trimmed: &str,
        abs: usize,
        quote: u8,
    ) -> (core::ops::Range<usize>, String, bool) {
        let b = trimmed.as_bytes();
        let mut value = String::new();
        let mut i = 1;
        while i < b.len() {
            match b[i] {
                // The escape consumes the closing candidate (double-quote
                // form).
                b'\\' if quote == b'"' && i + 1 < b.len() => match b[i + 1] {
                    b'"' | b'\\' => {
                        value.push(b[i + 1] as char);
                        i += 2;
                    }
                    // Unhandled escape: fail closed (no row for this
                    // scalar).
                    _ => return (abs..abs, String::new(), true),
                },
                c if c == quote => {
                    if quote == b'\'' && b.get(i + 1) == Some(&quote) {
                        value.push(quote as char);
                        i += 2;
                        continue;
                    }
                    return (abs..abs + i + 1, value, true);
                }
                _ => {
                    // Multi-byte UTF-8: copy the whole char. `i` is a char
                    // boundary by construction (every advance consumes a
                    // whole char or an ASCII escape/quote pair), so a
                    // `get` miss is a program bug — fail closed like the
                    // unhandled-escape arm instead of panicking.
                    let Some(ch) = trimmed.get(i..).and_then(|s| s.chars().next()) else {
                        return (abs..abs, String::new(), true);
                    };
                    value.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        (abs..abs, String::new(), true)
    }

    /// Whether a PLAIN scalar loads as a Dart `String` (`asString`
    /// :155-162 returns non-null only then): the `yaml` package's
    /// resolution (`loader.dart` `_tryParseScalar` :239-259) types a plain
    /// scalar as null/bool/number by first character and shape, and every
    /// other plain scalar loads as a String.
    fn yaml_plain_is_string(scalar: &str) -> bool {
        match scalar.as_bytes().first() {
            // Empty is null (the scan drops block form upstream of here).
            None => false,
            Some(&first) => match first {
                b'.' | b'+' | b'-' | b'0'..=b'9' => !yaml_number_shape(scalar),
                // `null`/`Null`/`NULL` (len 4), `~` (len 1).
                b'n' | b'N' => scalar.len() != 4,
                b't' | b'T' => scalar.len() != 4, // true/True/TRUE
                b'f' | b'F' => scalar.len() != 5, // false/False/FALSE
                b'~' => scalar.len() != 1,
                _ => true,
            },
        }
    }

    /// Whether the `yaml` package types the plain scalar as a number
    /// (`loader.dart` `_parseNumberValue` :281-379): single digits;
    /// `0x`-hex (lowercase gate, the loader's `secondChar == $x`);
    /// `0o`-octal; sign-or-digit-headed decimal/int shapes; and the
    /// `.`-headed infinity/NaN family. `false` leaves the scalar a
    /// String.
    fn yaml_number_shape(scalar: &str) -> bool {
        let b = scalar.as_bytes();
        if b.len() == 1 {
            return b[0].is_ascii_digit();
        }
        let (first, second) = (b[0], b[1]);
        if first == b'0' && second == b'x' {
            return dart_int_hex(b);
        }
        if first == b'0' && second == b'o' {
            return dart_int_radix(&b[2..], 8);
        }
        if first.is_ascii_digit() || ((first == b'+' || first == b'-') && second.is_ascii_digit()) {
            return dart_int_radix(b, 10) || dart_double_shape(b);
        }
        if (first == b'.' && second.is_ascii_digit())
            || ((first == b'-' || first == b'+') && second == b'.')
        {
            if b.len() == 5
                && matches!(
                    scalar,
                    "+.inf" | "+.Inf" | "+.INF" | "-.inf" | "-.Inf" | "-.INF"
                )
            {
                return true;
            }
            return dart_double_shape(b);
        }
        b.len() == 4
            && first == b'.'
            && matches!(scalar, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN")
    }

    /// Dart `int.tryParse` over an `0x` body (`loader.dart` :300-302
    /// delegates with the auto-radix parser): the `0x` prefix then hex
    /// digits, nothing else.
    fn dart_int_hex(b: &[u8]) -> bool {
        b.len() > 2 && b[2..].iter().all(u8::is_ascii_hexdigit)
    }

    /// Dart `int.tryParse(s, radix:)`: an optional sign then non-empty
    /// digits of the radix.
    fn dart_int_radix(b: &[u8], radix: u8) -> bool {
        let digits = match b.first() {
            Some(b'+') | Some(b'-') => &b[1..],
            _ => b,
        };
        !digits.is_empty()
            && digits
                .iter()
                .all(|&d| (d as char).to_digit(u32::from(radix)).is_some())
    }

    /// Dart `double.tryParse`'s decimal shape: an optional sign, digits
    /// with an optional fraction or a fraction alone, then an optional
    /// exponent (`1`, `1.`, `1.5`, `.5`, `1e5`, `1.5e-3`); the
    /// `Infinity`/`NaN` spellings never reach it here (the dispatch
    /// gates on digit/sign/dot heads).
    fn dart_double_shape(b: &[u8]) -> bool {
        let mut i = usize::from(matches!(b[0], b'+' | b'-'));
        let int_digits = (i..b.len()).take_while(|&j| b[j].is_ascii_digit()).count();
        i += int_digits;
        let mut digits = int_digits;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let fraction = (i..b.len()).take_while(|&j| b[j].is_ascii_digit()).count();
            i += fraction;
            digits += fraction;
        }
        if digits == 0 {
            return false;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            i += 1;
            i += usize::from(i < b.len() && matches!(b[i], b'+' | b'-'));
            let exponent = (i..b.len()).take_while(|&j| b[j].is_ascii_digit()).count();
            if exponent == 0 {
                return false;
            }
            i += exponent;
        }
        i == b.len()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn paths(text: &str) -> Vec<String> {
            scan_path_entries(text)
                .into_iter()
                .map(|entry| entry.path)
                .collect()
        }

        fn span(text: &str, needle: &str) -> core::ops::Range<usize> {
            let at = text.find(needle).unwrap();
            at..at + needle.len()
        }

        /// The verdict layering is the official order (:51-77): the POSIX
        /// gate first, then folder existence, then the folder's pubspec;
        /// a folder that exists with its pubspec stays silent.
        #[test]
        fn verdicts_layer_in_the_official_order() {
            assert_eq!(check_path_entry("vendor/meta", true, true), None);
            assert_eq!(
                check_path_entry("vendor/meta", false, false),
                Some(PathVerdict::DoesNotExist)
            );
            assert_eq!(
                check_path_entry("vendor/meta", true, false),
                Some(PathVerdict::PubspecDoesNotExist)
            );
            // The POSIX gate returns before any existence judgment.
            assert_eq!(
                check_path_entry(r"..\missing", false, false),
                Some(PathVerdict::NotPosix)
            );
        }

        /// The three templates render the oracle texts verbatim
        /// (`diagnostic.g.dart` `pathNotPosix` :14003-14011,
        /// `pathDoesNotExist` :13986-13995,
        /// `pathPubspecDoesNotExist` :14019-14029).
        #[test]
        fn templates_render_the_oracle_texts() {
            assert_eq!(
                PATH_NOT_POSIX.render_problem(&[r"..\windows"]),
                r"The path '..\windows' isn't a POSIX-style path."
            );
            assert_eq!(
                PATH_NOT_POSIX.render_correction(&[]).as_deref(),
                Some("Try converting the value to a POSIX-style path.")
            );
            assert_eq!(
                PATH_DOES_NOT_EXIST.render_problem(&["../missing_pkg"]),
                "The path '../missing_pkg' doesn't exist."
            );
            assert_eq!(
                PATH_DOES_NOT_EXIST.render_correction(&[]).as_deref(),
                Some("Try creating the referenced path or using a path that exists.")
            );
            assert_eq!(
                SPEC.render_problem(&["vendor/meta"]),
                "The directory 'vendor/meta' doesn't contain a pubspec."
            );
            assert_eq!(
                SPEC.render_correction(&[]).as_deref(),
                Some(
                    "Try creating a pubspec in the referenced directory or using a path \
                     that has a pubspec."
                )
            );
        }

        /// The corpus face: a nested package pubspec whose `path:` value
        /// node sits at line 13, column 11 — the raw value extent is the
        /// anchor (`reportErrorForNode(pathValue(), ..)`), `path:` at
        /// four spaces putting the value at byte 10 of the line.
        #[test]
        fn nested_pubspec_value_node_is_the_anchor() {
            let text = concat!(
                "name: fixture\n",
                "description: The nested package boundary's face.\n",
                "version: 1.0.0\n",
                "publish_to: none\n",
                "\n",
                "environment:\n",
                "  sdk: ^3.5.0\n",
                "\n",
                "dependencies:\n",
                "  meta:\n",
                "    path: vendor/meta\n",
                "  other:\n",
                "    path: ../missing_pkg\n",
            );
            let entries = scan_path_entries(text);
            assert_eq!(
                entries,
                vec![
                    PathEntry {
                        range: span(text, "vendor/meta"),
                        path: "vendor/meta".to_owned()
                    },
                    PathEntry {
                        range: span(text, "../missing_pkg"),
                        path: "../missing_pkg".to_owned()
                    },
                ]
            );
            // The report line reads `    path: ../missing_pkg` — the
            // value starts at byte 10 of line 13 (the 1-based column 11).
            let value_start = span(text, "../missing_pkg").start;
            let line_start = value_start - 10;
            assert!(text[line_start..].starts_with("    path: ../missing_pkg\n"));
        }

        /// Both judged sections record (`dependencies` :113-115 and
        /// `dev_dependencies` :117-128) and `dependency_overrides` never
        /// joins (the validator never reads it).
        #[test]
        fn both_sections_record_and_overrides_stay_silent() {
            let text = concat!(
                "dependencies:\n",
                "  meta:\n",
                "    path: vendor/meta\n",
                "dev_dependencies:\n",
                "  test:\n",
                "    path: vendor/test\n",
                "dependency_overrides:\n",
                "  broken:\n",
                "    path: vendor/broken\n",
            );
            assert_eq!(paths(text), vec!["vendor/meta", "vendor/test"]);
        }

        /// `dependency[PATH_FIELD]` reads the dep map's first level only:
        /// a `path:` nested under `git:` is a different field (:46).
        #[test]
        fn nested_path_keys_are_not_the_field() {
            let text = concat!(
                "dependencies:\n",
                "  foo:\n",
                "    git:\n",
                "      path: not_a_field\n",
                "    path: real/one\n",
            );
            assert_eq!(paths(text), vec!["real/one"]);
        }

        /// A dep entry's own line is never the spec (`path: ../x` as the
        /// dep NAME is a scalar value — not a YamlMap, the official
        /// returns :43-45) and a block-form `path:` is a null (`asString`
        /// null, silent :46-47).
        #[test]
        fn dep_line_and_empty_path_values_stay_silent() {
            assert_eq!(
                paths("dependencies:\n  path: ../not_a_spec\n"),
                Vec::<String>::new()
            );
            assert_eq!(
                paths("dependencies:\n  foo:\n    path:\n    other: x\n"),
                Vec::<String>::new()
            );
        }

        /// The `asString` gate (:155-162): plain scalars the yaml loader
        /// types as null/bool/number are not Strings — silent; quoted
        /// scalars are Strings whatever their text; plain scalars outside
        /// the typed shapes stay Strings (`loader.dart`
        /// `_tryParseScalar` :239-259).
        #[test]
        fn only_string_valued_paths_are_judged() {
            // yaml-typed non-strings: silent.
            for scalar in [
                "123", "0x1f", "0o17", "+5", "-3", "1.5", "5.", "1e5", "1.5e-3", ".inf", ".nan",
                "-.inf", "true", "False", "null", "~",
            ] {
                let text = format!("dependencies:\n  a:\n    path: {scalar}\n");
                assert!(paths(&text).is_empty(), "{scalar} must stay silent");
            }
            // Strings: quoted regardless of content, plain outside the
            // typed shapes (the resolution falls through to String).
            for scalar in ["12ab", "-x", "0X10", "0xZZ", "e5", "Infinity"] {
                let text = format!("dependencies:\n  a:\n    path: {scalar}\n");
                assert_eq!(paths(&text), vec![scalar], "{scalar} must record");
            }
        }

        /// A quoted value records with the quotes inside its span (the
        /// `YamlScalar.span` face) and the cooked text as the message arg.
        #[test]
        fn quoted_paths_span_the_quotes_and_cook() {
            let text = "dependencies:\n  foo:\n    path: 'a b'\n";
            let entries = scan_path_entries(text);
            assert_eq!(
                entries,
                vec![PathEntry {
                    range: span(text, "'a b'"),
                    path: "a b".to_owned()
                }]
            );
        }

        /// The resolution (:58-63): relative joins under the pubspec's
        /// directory, `..` pops (`pkg/../x` collapses to `x` — parsed_path
        /// :108-113 pops whatever the stack holds), `.` and empty parts
        /// drop, a rooted entry replaces the base, an absolute base roots
        /// the result, `..` past a relative start leads, `..` clamps at an
        /// absolute root, and a relative result collapsed to nothing
        /// reads `.` (:123-125).
        #[test]
        fn resolution_mirrors_the_official_path_face() {
            let dir = Path::new("/pkg/nested");
            assert_eq!(
                resolve_dependency_path(dir, "vendor/meta"),
                PathBuf::from("/pkg/nested/vendor/meta")
            );
            assert_eq!(
                resolve_dependency_path(dir, "../shared"),
                PathBuf::from("/pkg/shared")
            );
            assert_eq!(
                resolve_dependency_path(dir, "a//b/./c"),
                PathBuf::from("/pkg/nested/a/b/c")
            );
            assert_eq!(
                resolve_dependency_path(dir, "vendor/.."),
                PathBuf::from("/pkg/nested")
            );
            assert_eq!(
                resolve_dependency_path(dir, "."),
                PathBuf::from("/pkg/nested")
            );
            assert_eq!(
                resolve_dependency_path(dir, "/abs/x"),
                PathBuf::from("/abs/x")
            );
            assert_eq!(
                resolve_dependency_path(Path::new("pkg"), "../x"),
                PathBuf::from("x")
            );
            assert_eq!(
                resolve_dependency_path(Path::new("pkg"), "../../x"),
                PathBuf::from("../x")
            );
            assert_eq!(
                resolve_dependency_path(Path::new("pkg"), ".."),
                PathBuf::from(".")
            );
            assert_eq!(
                resolve_dependency_path(Path::new("/"), ".."),
                PathBuf::from("/")
            );
        }

        /// The `screenshotsValidator` walk (upstream Dart SDK source,
        /// `pkg/analyzer/lib/src/pubspec/validators/screenshot_validator.dart`
        /// :22-37): the section must be a `YamlList` (block form opens the
        /// section; an inline scalar never does), and each `YamlMap` item's
        /// `path` field records once with its value span and cooked text.
        #[test]
        fn screenshots_scan_picks_every_items_path_field() {
            let text = concat!(
                "screenshots:\n",
                "  - description: 'PetitParser'\n",
                "    path: petitparser.png\n",
                "  - path: other.png\n",
            );
            let entries = scan_screenshot_entries(text);
            assert_eq!(
                entries,
                vec![
                    PathEntry {
                        range: span(text, "petitparser.png"),
                        path: "petitparser.png".to_owned()
                    },
                    PathEntry {
                        range: span(text, "other.png"),
                        path: "other.png".to_owned()
                    },
                ]
            );
        }

        /// A `path:` deeper than the item's key-indent is a nested map's
        /// field — never the item's own `path` (the official reads
        /// `entry.valueAt(PATH_FIELD)` on the item's top-level map).
        #[test]
        fn screenshots_scan_skips_nested_map_path() {
            let text = concat!(
                "screenshots:\n",
                "  - description: x\n",
                "    nested:\n",
                "      path: not_the_field.png\n",
                "    path: real.png\n",
            );
            assert_eq!(
                scan_screenshot_entries(text)
                    .into_iter()
                    .map(|entry| entry.path)
                    .collect::<Vec<_>>(),
                vec!["real.png"]
            );
        }

        /// Inline `path:` on the `- ` dash line is the item's own field
        /// (`- path: a.png` is a one-key YamlMap) — same span/cook faces
        /// as the indented form.
        #[test]
        fn screenshots_scan_inline_path_is_the_field() {
            let text =
                "screenshots:\n  - path: inline.png\n  - description: d\n    path: next.png\n";
            assert_eq!(
                scan_screenshot_entries(text)
                    .into_iter()
                    .map(|entry| entry.path)
                    .collect::<Vec<_>>(),
                vec!["inline.png", "next.png"]
            );
        }

        /// `screenshots:` non-block scalar is not a `YamlList` — silent;
        /// a sibling top-level key closes the section, and a `path:`
        /// outside `screenshots:` is never this validator's field.
        #[test]
        fn screenshots_scan_stays_silent_outside_the_section() {
            assert!(scan_screenshot_entries("screenshots: none\n").is_empty());
            assert!(scan_screenshot_entries("path: x.png\n").is_empty());
            assert!(
                scan_screenshot_entries("screenshots:\n  - path: a.png\nother: 1\npath: b.png\n")
                    .into_iter()
                    .map(|entry| entry.path)
                    .eq(["a.png".to_owned()])
            );
        }
    }
}

pub(crate) mod secure_pubspec_urls {
    //! Official: `pkg/linter/lib/src/rules/pub/secure_pubspec_urls.dart` —
    //! pubspec URL values (the `homepage`/`repository`/`documentation`/
    //! `issue_tracker` entries and the `git`/`hosted` URLs of every
    //! `dependencies`/`dev_dependencies`/`dependency_overrides` entry,
    //! :24-58) must not use the `http` or `git` scheme (`_checkUrl`,
    //! :60-69). Message: `diagnostic.g.dart` `securePubspecUrls`.
    //!
    //! `kinds: &[]` — the verdict runs per pubspec value, not per AST node;
    //! the pubspec pass at the mounting face feeds the visited fields.

    use super::{RuleSpec, not_dispatched};

    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "secure_pubspec_urls",
    &[],
    not_dispatched,
);

    /// One insecure URL: the value's byte range in the pubspec text and the
    /// offending (lowercase) scheme — the message's `{0}`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InsecureUrl {
        pub range: core::ops::Range<usize>,
        pub scheme: &'static str,
    }

    /// The official `_checkUrl` (:60-69) over one URL value: the text
    /// parses as a URI (`Uri.tryParse` succeeds) whose scheme is `http`
    /// or `git` — push one row at `range`. The mounter walks the official
    /// visitor's fields (:24-58) and supplies each value's span (the
    /// official reports at the value node — the corpus spans are exactly
    /// the URL text).
    pub fn check_url(range: core::ops::Range<usize>, url: &str, out: &mut Vec<InsecureUrl>) {
        if let Some(scheme) = insecure_scheme(url) {
            out.push(InsecureUrl { range, scheme });
        }
    }

    /// The `Uri.tryParse` + `uri.isScheme` face (`sdk/lib/core/uri.dart`):
    /// an RFC 3986 scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"
    /// — `_makeScheme`, :2628-2645) at the head, then the full-parse
    /// validity faces that make `Uri.tryParse` return `null` — a broken
    /// authority (`_makeHost` :2407-2520: unclosed `[`, invalid IPv6
    /// literal per `parseIPv6Address` :1658, character after `]` not `:`;
    /// `_normalize` :2889-2931 "Invalid character" on the general
    /// delimiters `[`, `]`, `@` in the userinfo and the reg-name — the
    /// only components normalized with `escapeDelimiters: false`;
    /// path/query/fragment normalize with `escapeDelimiters: true`
    /// :2690/:2733/:2784 and never fail) or an invalid port
    /// (`int.tryParse`, `notSimple` :1908-1911). Returns the lowercased
    /// scheme only when it is one of the two insecure ones and the whole
    /// text parses; scheme-less texts (`github.com/x`, scp-style
    /// `git@host:repo`, relative paths) and secure schemes are `None`
    /// regardless of validity, since the verdict never reaches them.
    fn insecure_scheme(uri: &str) -> Option<&'static str> {
        let b = uri.as_bytes();
        if b.is_empty() || !b[0].is_ascii_alphabetic() {
            return None;
        }
        let mut i = 1;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'+' | b'-' | b'.')) {
            i += 1;
        }
        if i == b.len() || b[i] != b':' {
            return None;
        }
        let scheme = if i == 4 && b[..4].eq_ignore_ascii_case(b"http") {
            "http"
        } else if i == 3 && b[..3].eq_ignore_ascii_case(b"git") {
            "git"
        } else {
            return None;
        };
        if parses_as_uri(&b[i + 1..]) {
            Some(scheme)
        } else {
            None
        }
    }

    /// The `Uri.parse` failure faces over the text after the scheme colon.
    /// Percent escapes never fail (they are re-escaped, `_normalize`
    /// :2916-2923), and path/query/fragment never fail either — they
    /// normalize with `escapeDelimiters: true` (`_makePath` :2685-2692,
    /// `_makeQuery` :2728-2734, `_makeFragment` :2779-2785), so even the
    /// general delimiters `[` and `]` are percent-escaped there rather
    /// than rejected; every reachable failure is in the authority below.
    fn parses_as_uri(rest: &[u8]) -> bool {
        if let Some(authority) = rest.strip_prefix(b"//") {
            let end = authority
                .iter()
                .position(|&c| matches!(c, b'/' | b'?' | b'#'))
                .unwrap_or(authority.len());
            parses_as_authority(&authority[..end])
        } else {
            true
        }
    }

    /// The authority face: `[userinfo "@"] host [":" port]`. Userinfo
    /// (`_normalize` with `_userinfoMask`) fails on `[`/`]`; the host
    /// (`_makeHost` :2407-2520) is either a bracketed IP literal — the
    /// `]` must close before the authority ends and only `:` may follow —
    /// or a reg-name whose general delimiters (`[`, `]`, `@`) fail; the
    /// port text must satisfy `int.tryParse` (`notSimple` :1908-1911).
    fn parses_as_authority(authority: &[u8]) -> bool {
        let at = authority.iter().position(|&c| c == b'@');
        let (userinfo, hostport) = match at {
            Some(i) => (&authority[..i], &authority[i + 1..]),
            None => (&authority[..0], authority),
        };
        if userinfo.iter().any(|&c| c == b'[' || c == b']') {
            return false;
        }
        if let Some(rest) = hostport.strip_prefix(b"[") {
            let Some(close) = rest.iter().position(|&c| c == b']') else {
                return false;
            };
            if !parses_as_ip_literal(&rest[..close]) {
                return false;
            }
            let after = &rest[close + 1..];
            match after.split_first() {
                None => true,
                Some((&b':', port)) => parses_as_port(port),
                Some(_) => false,
            }
        } else {
            match hostport.iter().position(|&c| c == b':') {
                Some(i) => parses_as_reg_name(&hostport[..i]) && parses_as_port(&hostport[i + 1..]),
                None => parses_as_reg_name(hostport),
            }
        }
    }

    /// Bracketed host content: IPvFuture (`v` 1*HEXDIG "." 1*address-char,
    /// `_validateIPvFutureAddress`) or IPv6 with an optional RFC 6874 zone
    /// after the first `%` (any further `%` in the zone must open a valid
    /// escape — `_normalizeZoneID`'s "ZoneID should not contain % anymore").
    fn parses_as_ip_literal(content: &[u8]) -> bool {
        if content.first() == Some(&b'v') {
            let mut i = 1;
            while i < content.len() && content[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i == 1 || i == content.len() || content[i] != b'.' {
                return false;
            }
            return content[i + 1..].iter().all(|&c| {
                c.is_ascii_alphanumeric()
                    || matches!(
                        c,
                        b'-' | b'.'
                            | b'_'
                            | b'~'
                            | b'!'
                            | b'$'
                            | b'&'
                            | b'\''
                            | b'('
                            | b')'
                            | b'*'
                            | b'+'
                            | b','
                            | b';'
                            | b'='
                            | b':'
                    )
            }) && !content[i + 1..].is_empty();
        }
        match content.iter().position(|&c| c == b'%') {
            None => parses_as_ipv6(content),
            Some(i) => {
                if !parses_as_ipv6(&content[..i]) {
                    return false;
                }
                let zone = &content[i + 1..];
                let mut j = 0;
                while j < zone.len() {
                    if zone[j] == b'%' {
                        if j + 2 >= zone.len()
                            || !zone[j + 1].is_ascii_hexdigit()
                            || !zone[j + 2].is_ascii_hexdigit()
                        {
                            return false;
                        }
                        j += 3;
                    } else {
                        j += 1;
                    }
                }
                true
            }
        }
    }

    /// `parseIPv6Address` (:1658-1790): 8 hex parts of 1-4 digits
    /// separated by `:`, at most one `::` wildcard, an optional trailing
    /// dotted-quad IPv4 tail (decimal parts only, no leading zeros).
    fn parses_as_ipv6(b: &[u8]) -> bool {
        if b.len() < 2 {
            return false;
        }
        let mut cursor = 0;
        let mut part_start;
        let mut wildcard_at: isize = -1;
        let mut part_count = 0;
        let mut hex_value: u32 = 0;
        let mut dec_value = true;
        if b[0] == b':' {
            if b[1] != b':' {
                return false;
            }
            wildcard_at = 0;
            part_count = 1;
            part_start = 2;
            cursor = 2;
        } else {
            part_start = 0;
        }
        loop {
            let ch = if cursor >= b.len() { 0 } else { b[cursor] };
            let hex_digit = match ch {
                c @ b'0'..=b'9' => Some((c - b'0') as u32),
                c @ b'a'..=b'f' => {
                    dec_value = false;
                    Some((c - b'a' + 10) as u32)
                }
                c @ b'A'..=b'F' => {
                    dec_value = false;
                    Some((c - b'A' + 10) as u32)
                }
                _ => None,
            };
            if let Some(digit) = hex_digit {
                if cursor < part_start + 4 {
                    hex_value = hex_value * 16 + digit;
                    cursor += 1;
                    continue;
                }
                return false;
            }
            if cursor > part_start {
                if ch == b'.' {
                    if dec_value {
                        if part_count > 6 || !parses_as_ipv4(&b[part_start..]) {
                            return false;
                        }
                        part_count += 2;
                        cursor = b.len();
                        break;
                    }
                    break;
                }
                part_count += 1;
                if ch == b':' {
                    if part_count < 8 {
                        hex_value = 0;
                        dec_value = true;
                        cursor += 1;
                        part_start = cursor;
                        continue;
                    }
                    return false;
                }
                break;
            }
            if ch == b':' {
                if wildcard_at >= 0 {
                    return false;
                }
                wildcard_at = part_count as isize;
                part_count += 1;
                cursor += 1;
                part_start = cursor;
                continue;
            }
            if wildcard_at != part_count as isize - 1 {
                return false;
            }
            break;
        }
        cursor >= b.len() && (part_count == 8 || wildcard_at >= 0)
    }

    /// `_parseIPv4Address` (:1450-1520): exactly 4 decimal octets in
    /// 0..255, no leading zeros.
    fn parses_as_ipv4(b: &[u8]) -> bool {
        let mut cursor = 0;
        let mut octet_start = 0;
        let mut octet_index = 0;
        let mut octet_value: u32 = 0;
        loop {
            let ch = if cursor >= b.len() { 0 } else { b[cursor] };
            if ch.is_ascii_digit() {
                if octet_value != 0 || cursor == octet_start {
                    octet_value = octet_value * 10 + (ch - b'0') as u32;
                    if octet_value <= 255 {
                        cursor += 1;
                        continue;
                    }
                    return false;
                }
                return false;
            }
            if cursor == octet_start {
                if cursor == b.len() {
                    break;
                }
                return false;
            }
            octet_index += 1;
            if ch == b'.' {
                if octet_index < 4 {
                    octet_value = 0;
                    cursor += 1;
                    octet_start = cursor;
                    continue;
                }
                break;
            }
            if cursor == b.len() {
                return octet_index == 4;
            }
            return false;
        }
        false
    }

    /// `int.tryParse` on the port text (`notSimple` :1908-1911): an
    /// optional sign then decimal digits, or a `0x`/`0X` hex literal; the
    /// empty port is omitted before this runs (`portStart + 1 == pathStart`,
    /// :1009-1012) so it stays valid.
    fn parses_as_port(port: &[u8]) -> bool {
        if port.is_empty() {
            return true;
        }
        let body = match port.split_first() {
            Some((b'+' | b'-', rest)) => {
                if rest.is_empty() {
                    return false;
                }
                rest
            }
            _ => port,
        };
        if body.len() >= 2 && body[0] == b'0' && (body[1] | 0x20) == b'x' {
            return !body[2..].is_empty() && body[2..].iter().all(|c| c.is_ascii_hexdigit());
        }
        body.iter().all(|c| c.is_ascii_digit())
    }

    /// Reg-name host general delimiters (`_normalizeRegName` :2597-2600):
    /// of the RFC 3986 gen-delims only `[`, `]` and `@` can reach the host
    /// substring — `/`, `?`, `#` ended the authority and the first `:`
    /// split the port — and each fails the parse.
    fn parses_as_reg_name(host: &[u8]) -> bool {
        !host.iter().any(|&c| matches!(c, b'[' | b']' | b'@'))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn hits(url: &str) -> Vec<InsecureUrl> {
            let mut out = Vec::new();
            check_url(0..url.len(), url, &mut out);
            out
        }

        /// The official `test_dependencyGit_insecure` corpus value.
        #[test]
        fn http_reports_with_the_scheme_arg() {
            assert_eq!(
                hits("http://github.com/munificent/kittens.git"),
                vec![InsecureUrl {
                    range: 0..40,
                    scheme: "http"
                }]
            );
        }

        /// The `git` scheme is insecure too (`uri.isScheme('git')`).
        #[test]
        fn git_scheme_reports() {
            assert_eq!(
                hits("git://github.com/munificent/kittens.git")[0].scheme,
                "git"
            );
        }

        /// The official `test_homepage_secure`.
        #[test]
        fn https_is_silent() {
            assert!(hits("https://github.com/dart-lang/linter").is_empty());
        }

        /// `Uri.scheme` normalizes to lowercase (`isScheme` is
        /// case-insensitive).
        #[test]
        fn scheme_is_case_normalized() {
            assert_eq!(hits("HTTP://a.dev")[0].scheme, "http");
        }

        /// Scheme-less texts parse without a scheme — no report.
        #[test]
        fn schemeless_texts_stay_silent() {
            assert!(hits("git@github.com:munificent/kittens.git").is_empty());
            assert!(hits("github.com/munificent/kittens").is_empty());
            assert!(hits("../local/pkg").is_empty());
            assert!(hits("").is_empty());
        }

        #[test]
        fn other_schemes_stay_silent() {
            assert!(hits("ssh://git@github.com/x/y.git").is_empty());
            assert!(hits("httpx://a.dev").is_empty());
        }

        /// `Uri.tryParse` returns null on a broken authority — an
        /// unclosed `[` host ("Missing end `]`"), a malformed IPv6
        /// literal, a general delimiter in the reg-name — and on a port
        /// that `int.tryParse` rejects; none of these report.
        #[test]
        fn malformed_uris_stay_silent() {
            assert!(hits("http://[::1").is_empty());
            assert!(hits("git://[fe80::1:x").is_empty());
            assert!(hits("http://[gg::1]/repo").is_empty());
            assert!(hits("http://[12345::1]/repo").is_empty());
            assert!(hits("http://[::1]:notaport/repo").is_empty());
            assert!(hits("http://host:notaport/repo").is_empty());
            assert!(hits("http://host:x80/repo").is_empty());
            assert!(hits("http://a@b@c/repo").is_empty());
            assert!(hits("http://ho[st/repo").is_empty());
            assert!(hits("http://[::1]x/repo").is_empty());
            assert!(hits("http://[::1]./repo").is_empty());
            assert!(hits("http://[::1%25zz%ww]/repo").is_empty());
        }

        /// Bracketed IPv6 hosts, an empty port and an IPv6 zone are
        /// legal parses — they report like any other `http` URL. So are
        /// a valid IPvFuture host (`v1.a` = version `1`, address `a`)
        /// and `[`/`]` in the path or query: those components normalize
        /// with `escapeDelimiters: true` and escape the delimiters
        /// instead of failing.
        #[test]
        fn valid_malformed_looking_uris_report() {
            assert_eq!(hits("http://[::1]/repo")[0].scheme, "http");
            assert_eq!(hits("http://[fe80::1%25eth0]/repo")[0].scheme, "http");
            assert_eq!(hits("http://host:/repo")[0].scheme, "http");
            assert_eq!(hits("http://[2001:db8::1]:8080/repo")[0].scheme, "http");
            assert_eq!(hits("http://[::1%zz2]/repo")[0].scheme, "http");
            assert_eq!(hits("git://[::ffff:1.2.3.4]/r")[0].scheme, "git");
            assert_eq!(hits("http://host:0x50/repo")[0].scheme, "http");
            assert_eq!(hits("http://[v1.a]/repo")[0].scheme, "http");
            assert_eq!(hits("http://host/re[po")[0].scheme, "http");
            assert_eq!(hits("http:re[po")[0].scheme, "http");
            assert_eq!(hits("http://[::1]/re?po[x")[0].scheme, "http");
        }
    }
}

pub(crate) mod package_names {
    //! Official: `pkg/linter/lib/src/rules/pub/package_names.dart` — the
    //! pubspec `name:` value must satisfy `isValidPackageName`
    //! (`pkg/linter/lib/src/utils.dart:84-88`): the
    //! `lowercase_with_underscores` shape (`utils.dart:44-46`), a legal
    //! Dart identifier (`utils.dart:20`), and no reserved word
    //! (`utils.dart:48-50` — the `Keyword.keywords` entries whose
    //! `isReservedWord` holds). Message: `diagnostic.g.dart`
    //! `packageNames`.
    //!
    //! `kinds: &[]` — the verdict runs per pubspec value, not per AST node;
    //! the pubspec pass at the mounting face feeds the `name:` entry.

    use super::{RuleSpec, not_dispatched};

    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "package_names",
    &[],
    not_dispatched,
);

    /// One invalid name: the value's byte range in the pubspec text and the
    /// name itself — the message's `{0}`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InvalidName {
        pub range: core::ops::Range<usize>,
        pub name: String,
    }

    /// The official `visitPackageName` (package_names.dart:26-32) over one
    /// `name:` value: an invalid name pushes one row at `range`. The
    /// mounter locates the `name` entry and supplies its value span (the
    /// official reports at the value node).
    pub fn check_name(range: core::ops::Range<usize>, name: &str, out: &mut Vec<InvalidName>) {
        if !is_valid_package_name(name) {
            out.push(InvalidName {
                range,
                name: name.to_owned(),
            });
        }
    }

    /// The official `isValidPackageName` (utils.dart:85-88).
    #[must_use]
    pub fn is_valid_package_name(name: &str) -> bool {
        is_lower_case_with_underscores(name) && is_identifier(name) && !is_reserved_word(name)
    }

    /// `utils.dart:44-46`: `^_*[a-z](?:_?[a-z\d])*$` — any run of leading
    /// underscores, a lowercase letter, then optional single-underscore
    /// separators before further lowercase letters / digits (no two
    /// adjacent underscores, no trailing underscore).
    fn is_lower_case_with_underscores(name: &str) -> bool {
        let b = name.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i] == b'_' {
            i += 1;
        }
        if i >= b.len() || !b[i].is_ascii_lowercase() {
            return false;
        }
        i += 1;
        while i < b.len() {
            if b[i] == b'_' {
                i += 1;
                if i >= b.len() || !(b[i].is_ascii_lowercase() || b[i].is_ascii_digit()) {
                    return false;
                }
            } else if !(b[i].is_ascii_lowercase() || b[i].is_ascii_digit()) {
                return false;
            }
            i += 1;
        }
        true
    }

    /// `utils.dart:20` (case-insensitive): `^[_$a-z]+(\d[_a-z\d]*)?$` — a
    /// run of `_`/`$`/letters, then optionally a digit followed by any
    /// `_`/alphanumeric run.
    fn is_identifier(name: &str) -> bool {
        let b = name.as_bytes();
        let mut i = 0;
        while i < b.len() && (b[i] == b'_' || b[i] == b'$' || b[i].is_ascii_alphabetic()) {
            i += 1;
        }
        if i == 0 {
            return false;
        }
        // The optional `(\d[_a-z\d]*)?` tail: whatever is left must start
        // with a digit.
        if i == b.len() {
            return true;
        }
        if !b[i].is_ascii_digit() {
            return false;
        }
        b[i + 1..]
            .iter()
            .all(|&c| c == b'_' || c.is_ascii_alphanumeric())
    }

    /// `utils.dart:48-50` — the 33 `KeywordStyle.reserved` entries of
    /// `Keyword.keywords` (`pkg/_fe_analyzer_shared/lib/src/scanner/
    /// token.dart`; built-in identifiers like `dynamic` are pseudo
    /// keywords and stay valid).
    fn is_reserved_word(name: &str) -> bool {
        matches!(
            name,
            "assert"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "default"
                | "do"
                | "else"
                | "enum"
                | "extends"
                | "false"
                | "final"
                | "finally"
                | "for"
                | "if"
                | "in"
                | "is"
                | "new"
                | "null"
                | "rethrow"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "var"
                | "void"
                | "while"
                | "with"
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The official corpus: `test_oneWord` / `test_snakeCase` pass;
        /// `test_lowerCamelCase` / `test_oneUpperWord` /
        /// `test_upperCamelCase` report.
        #[test]
        fn official_corpus() {
            assert!(is_valid_package_name("foo"));
            assert!(is_valid_package_name("foo_bar"));
            assert!(!is_valid_package_name("fooBar"));
            assert!(!is_valid_package_name("Foo"));
            assert!(!is_valid_package_name("FooBar"));
        }

        /// The `^_*[a-z](?:_?[a-z\d])*$` shape edges.
        #[test]
        fn underscore_edges_follow_the_official_regex() {
            assert!(is_valid_package_name("_foo"));
            assert!(!is_valid_package_name("foo_"));
            assert!(!is_valid_package_name("foo__bar"));
            assert!(!is_valid_package_name("_"));
            assert!(!is_valid_package_name(""));
            assert!(!is_valid_package_name("1foo"));
            assert!(is_valid_package_name("foo2"));
            assert!(is_valid_package_name("foo_2"));
            assert!(!is_valid_package_name("foo-bar"));
        }

        /// The identifier face (`^[_$a-z]+(\d[_a-z\d]*)?$`) and the
        /// reserved-word face (`utils.dart:48-50`).
        #[test]
        fn identifier_and_reserved_word_faces() {
            assert!(is_valid_package_name("a1_b"));
            assert!(!is_valid_package_name("a1_"));
            assert!(!is_valid_package_name("class"));
            assert!(!is_valid_package_name("switch"));
            // Built-in identifiers are pseudo keywords, not reserved.
            assert!(is_valid_package_name("dynamic"));
            assert!(is_valid_package_name("async"));
        }

        /// The row carries the value range and the verbatim name (the
        /// official `arguments: [packageName]`).
        #[test]
        fn check_name_emits_the_row() {
            let mut out = Vec::new();
            check_name(6..12, "fooBar", &mut out);
            assert_eq!(
                out,
                vec![InvalidName {
                    range: 6..12,
                    name: "fooBar".to_owned()
                }]
            );
            out.clear();
            check_name(0..3, "foo", &mut out);
            assert!(out.is_empty());
        }
    }
}

pub(crate) mod depend_on_referenced_packages {
    //! Official: `pkg/linter/lib/src/rules/pub/depend_on_referenced_packages.dart`
    //! — an import or export whose URI is a plain `package:<name>/…`
    //! string, whose `<name>` is neither the importing package itself nor
    //! one of its (lib-visible) dependencies, reports at the URI node
    //! (:58-79; the available set :44-55 — the own name, `dependencies`,
    //! and `dev_dependencies` outside the public dirs, `isInPublicDir`,
    //! ast.dart:112-129). Virtual packages (`flutter_gen`, :68-72) skip.
    //!
    //! Mounted on the AST dispatch off [`WorkspaceFacts`][crate::WorkspaceFacts]
    //! (`Unit.workspace`): `None` is the official no-pubspec registration
    //! gate (:33-40), keeping the rule silent. Message:
    //! `diagnostic.g.dart` `dependOnReferencedPackages`.

    use super::{NodeKind, RuleSpec};

    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "depend_on_referenced_packages",
    &[NodeKind::Import, NodeKind::Export],
    check,
);

    /// The virtual packages that never report (:68-72).
    const VIRTUAL_PACKAGES: [&str; 1] = ["flutter_gen"];

    fn check(c: &mut crate::engine::Check<'_>) {
        let Some(facts) = c.unit.workspace else {
            return;
        };
        // Dev dependencies drop out of the available set in the public
        // dirs (:47-55, isInPublicDir). A pathless run reads as outside
        // them (dev deps available) — the official `cuPath == null` arm.
        let public = c
            .unit
            .path
            .is_some_and(|path| is_in_public_dir(path, &facts.root));
        let Some((range, uri)) = crate::rules::directives::written_uri(c, c.node()) else {
            return;
        };
        let dev = (!public).then_some(&facts.dev_dependencies);
        if let Some(package) = check_uri(&facts.dependencies, dev, &uri) {
            c.report(range, &[&package]);
        }
    }

    /// The official `_checkDirective` (:58-79) over one directive URI:
    /// `Some(package name)` reports at the URI node. `available` is the
    /// own name plus the `dependencies` entries; `dev_available` carries
    /// the `dev_dependencies` entries for units outside the public dirs
    /// (the caller applies `isInPublicDir`).
    #[must_use]
    pub fn check_uri(
        available: &rustc_hash::FxHashSet<String>,
        dev_available: Option<&rustc_hash::FxHashSet<String>>,
        uri: &str,
    ) -> Option<String> {
        let rest = uri.strip_prefix("package:")?;
        let first_slash = rest.find('/')?;
        let package_name = &rest[..first_slash];
        if VIRTUAL_PACKAGES.contains(&package_name) {
            return None;
        }
        if available.contains(package_name)
            || dev_available.is_some_and(|dev| dev.contains(package_name))
        {
            return None;
        }
        Some(package_name.to_owned())
    }

    /// `isInPublicDir` (ast.dart:112-129): inside the package's `lib/` or
    /// `bin/`, or one of the two hook files at the root.
    pub(crate) fn is_in_public_dir(path: &std::path::Path, root: &std::path::Path) -> bool {
        path.starts_with(root.join("lib"))
            || path.starts_with(root.join("bin"))
            || path == root.join("hook").join("build.dart")
            || path == root.join("hook").join("link.dart")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn set(names: &[&str]) -> rustc_hash::FxHashSet<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        }

        /// A dependency import stays silent; a foreign package reports.
        #[test]
        fn foreign_package_reports() {
            let available = set(&["own", "meta"]);
            assert_eq!(
                check_uri(&available, None, "package:foreign/foreign.dart"),
                Some("foreign".to_owned())
            );
            assert_eq!(check_uri(&available, None, "package:meta/meta.dart"), None);
        }

        /// The own package's imports never report (:44-55).
        #[test]
        fn own_package_imports_stay_silent() {
            assert_eq!(
                check_uri(&set(&["own"]), None, "package:own/src/inner.dart"),
                None
            );
        }

        /// Dev dependencies count only where the caller passed them (the
        /// public-dir gate is the mounter's, :47-55).
        #[test]
        fn dev_dependencies_honor_the_public_dir_gate() {
            let dev = set(&["test"]);
            assert_eq!(
                check_uri(&set(&["own"]), Some(&dev), "package:test/test.dart"),
                None
            );
            assert_eq!(
                check_uri(&set(&["own"]), None, "package:test/test.dart"),
                Some("test".to_owned())
            );
        }

        /// Non-`package:` URIs (SDK, relative, truncated) never report
        /// (:61-64).
        #[test]
        fn non_package_uris_stay_silent() {
            let available = set(&["own"]);
            assert_eq!(check_uri(&available, None, "dart:core"), None);
            assert_eq!(check_uri(&available, None, "foo.dart"), None);
            assert_eq!(check_uri(&available, None, "package:noname"), None);
        }

        /// Virtual packages skip (:68-72).
        #[test]
        fn virtual_packages_skip() {
            assert_eq!(
                check_uri(&set(&["own"]), None, "package:flutter_gen/gen.dart"),
                None
            );
        }

        /// The public-dir gate (`isInPublicDir`, ast.dart:112-129): under
        /// `lib/` or `bin/`, or one of the two hook files.
        #[test]
        fn public_dir_gate() {
            let root = std::path::Path::new("/pkg");
            for public in [
                "/pkg/lib/a.dart",
                "/pkg/bin/run.dart",
                "/pkg/hook/build.dart",
            ] {
                assert!(
                    is_in_public_dir(std::path::Path::new(public), root),
                    "{public}"
                );
            }
            for private in ["/pkg/test/a.dart", "/pkg/lib2/a.dart", "/other/lib/a.dart"] {
                assert!(
                    !is_in_public_dir(std::path::Path::new(private), root),
                    "{private}"
                );
            }
        }
    }
}

pub(crate) mod sort_pub_dependencies {
    //! Official: `pkg/linter/lib/src/rules/pub/sort_pub_dependencies.dart` —
    //! the `dependencies`, `dependency_overrides`, and `dev_dependencies`
    //! sections of `pubspec.yaml` are each checked alphabetically (the three
    //! visitor methods :27-39 share `_visitDeps`, :41-65): walking the
    //! section's entries in name-span document order (the :49-50 location
    //! sort — the scan face's natural order), the first key whose text
    //! compares less than the previous key's text (Dart `String.compareTo`,
    //! :57) reports at the name node (:58) and the walk returns — one
    //! diagnostic per section, `previousName` fresh per section (:51).
    //! Entries the analyzer model leaves unnamed (:53-63) neither report nor
    //! advance `previousName` — the mounting face filters them out before
    //! feeding. Message: `diagnostic.g.dart` `sortPubDependencies`.
    //!
    //! `kinds: &[]` — the verdict runs per section entry list, not per AST
    //! node; the pubspec pass at the mounting face feeds each section's
    //! named entries in document order with their name spans.

    use super::{RuleSpec, not_dispatched};

    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "sort_pub_dependencies",
    &[],
    not_dispatched,
);

    /// The official `_visitDeps` (:41-65) over one section's named entries
    /// in document order — `(name span, name text)` pairs as the pubspec
    /// scan yields them. `Some(span)` is the first out-of-order name; the
    /// official reports at the name node and returns (:58-59), so the row
    /// span is exactly the name text (the corpus `lint(86, 3)`) and at
    /// most one diagnostic leaves a section. Each section is fed
    /// separately — the official's three visitor calls each start from
    /// `previousName = ''` (:51), so the caller re-enters per section.
    #[must_use]
    pub fn check_section(
        entries: &[(core::ops::Range<usize>, &str)],
    ) -> Option<core::ops::Range<usize>> {
        let mut previous = "";
        for (range, name) in entries {
            if dart_compare(name, previous) == core::cmp::Ordering::Less {
                return Some(range.clone());
            }
            previous = name;
        }
        None
    }

    /// Dart `String.compareTo` (:57): lexicographic over UTF-16 code units
    /// — case-sensitive, uppercase before lowercase. Rust `str`'s byte
    /// order is code-point order, which disagrees with code-unit order
    /// only for supplementary-plane characters against BMP `U+E000..`
    /// `U+FFFF`, so the comparison runs over the UTF-16 encoding.
    fn dart_compare(a: &str, b: &str) -> core::cmp::Ordering {
        a.encode_utf16().cmp(b.encode_utf16())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// One `(span, text)` pair per name, spans laid out with two
        /// separator bytes between entries (`name: value` lines) — the
        /// spans' gaps play no role in the verdict.
        fn entries<'a>(names: &[&'a str]) -> Vec<(core::ops::Range<usize>, &'a str)> {
            let mut out = Vec::new();
            let mut at = 0;
            for name in names {
                out.push((at..at + name.len(), *name));
                at += name.len() + 2;
            }
            out
        }

        /// The official `test_dependencies_sorted`.
        #[test]
        fn sorted_dependencies_stay_silent() {
            assert_eq!(check_section(&entries(&["aaa", "bbb", "flutter"])), None);
        }

        /// The official `test_dependencies_unsorted`: the report lands at
        /// the first out-of-order key, its span exactly the name text
        /// (the corpus `lint(86, 3)` — the name node, not the entry line).
        #[test]
        fn unsorted_dependencies_report_at_the_first_out_of_order_name() {
            let text = concat!(
                "name: fancy\nversion: 1.1.1\n\ndependencies:\n",
                "  aaa: ^0.1.1\n  flutter:\n    sdk: flutter\n  bbb: ^0.15.8\n"
            );
            let span = |needle: &str| {
                let at = text.find(needle).unwrap();
                at..at + needle.len()
            };
            let section = vec![
                (span("aaa"), "aaa"),
                (span("flutter"), "flutter"),
                (span("bbb"), "bbb"),
            ];
            assert_eq!(check_section(&section), Some(span("bbb")));
        }

        /// The official `test_devDependencies_unsorted` /
        /// `test_dependencyOverrides_unsorted`: every section is judged,
        /// and a sorted section stays silent while another reports — each
        /// call is one section with a fresh `previousName`.
        #[test]
        fn sections_are_judged_independently() {
            let dev = entries(&["aaa", "flutter", "bbb"]);
            let deps = entries(&["aaa", "bbb", "flutter"]);
            assert_eq!(check_section(&dev), Some(dev[2].0.clone()));
            assert_eq!(check_section(&deps), None);
        }

        /// The official `test_dependencies_duplicates`: an equal adjacent
        /// key compares 0, not less.
        #[test]
        fn duplicate_keys_pass() {
            assert_eq!(check_section(&entries(&["aaa", "aaa", "bbb"])), None);
        }

        /// Dart `String.compareTo` is case-sensitive over code units —
        /// uppercase sorts before lowercase (`Z` is 0x5A, `a` is 0x61).
        #[test]
        fn comparison_is_case_sensitive() {
            assert_eq!(check_section(&entries(&["Zebra", "apple"])), None);
            let hits = entries(&["apple", "Zebra"]);
            assert_eq!(check_section(&hits), Some(hits[1].0.clone()));
        }

        /// Comments and blank lines between entries leave the judgment to
        /// the key texts alone — the analyzer model is comment-agnostic,
        /// so offset gaps between fed entries never matter.
        #[test]
        fn comments_between_entries_do_not_break_the_judgment() {
            let text = concat!(
                "dependencies:\n  aaa: ^0.1.1\n  # a comment\n\n",
                "  flutter:\n    sdk: flutter\n  bbb: ^0.15.8\n"
            );
            let span = |needle: &str| {
                let at = text.find(needle).unwrap();
                at..at + needle.len()
            };
            let section = vec![
                (span("aaa"), "aaa"),
                (span("flutter"), "flutter"),
                (span("bbb"), "bbb"),
            ];
            assert_eq!(check_section(&section), Some(span("bbb")));
        }

        /// Only the first violation reports — the official returns at
        /// :58-59.
        #[test]
        fn only_the_first_out_of_order_name_reports() {
            let hits = entries(&["zzz", "mmm", "aaa"]);
            assert_eq!(check_section(&hits), Some(hits[1].0.clone()));
        }

        /// `previousName` starts at `''` (:51) — a section's first entry
        /// never reports, whatever its text; one-entry and empty sections
        /// pass.
        #[test]
        fn first_entry_never_reports() {
            assert_eq!(check_section(&entries(&["zzz"])), None);
            assert_eq!(check_section(&entries(&["flutter"])), None);
            assert_eq!(check_section(&[]), None);
        }
    }
}
