//! The pubspec family: `secure_pubspec_urls` (core), `package_names`
//! (recommended), and `depend_on_referenced_packages` (core) — the official
//! home is `pkg/linter/lib/src/rules/pub/`.
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

    use super::{Group, RuleSpec, not_dispatched};

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "secure_pubspec_urls",
        group: Group::Core,
        problem: "The '{0}' protocol shouldn't be used because it isn't secure.",
        correction: "Try using a secure protocol, such as 'https'.",
        kinds: &[],
        check: not_dispatched,
    };

    /// One insecure URL: the value's byte range in the pubspec text and the
    /// offending (lowercase) scheme — the message's `{0}`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InsecureUrl {
        pub range: core::ops::Range<usize>,
        pub scheme: &'static str,
    }

    /// The official `_checkUrl` (:60-69) over one URL value: the text
    /// parses as a URI whose scheme is `http` or `git` — push one row at
    /// `range`. The mounter walks the official visitor's fields (:24-58)
    /// and supplies each value's span (the official reports at the value
    /// node — the corpus spans are exactly the URL text).
    ///
    /// Divergence face (documented): `Uri.tryParse` additionally rejects a
    /// handful of malformed URIs (broken IPv6 literals, invalid ports)
    /// whose scheme prefix this extraction still reads — such a value
    /// reports where the official stays silent, on a string that is no
    /// working URL either way.
    pub fn check_url(range: core::ops::Range<usize>, url: &str, out: &mut Vec<InsecureUrl>) {
        if let Some(scheme) = scheme_of(url) {
            out.push(InsecureUrl { range, scheme });
        }
    }

    /// The `Uri.tryParse` + `uri.scheme` face: an RFC 3986 scheme
    /// (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`) at the head,
    /// lowercased; `None` when the text is scheme-less (`github.com/x`,
    /// scp-style `git@host:repo`, a relative path) or the scheme is a
    /// secure one.
    fn scheme_of(uri: &str) -> Option<&'static str> {
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
        if i == 4 && b[..4].eq_ignore_ascii_case(b"http") {
            return Some("http");
        }
        if i == 3 && b[..3].eq_ignore_ascii_case(b"git") {
            return Some("git");
        }
        None
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

    use super::{Group, RuleSpec, not_dispatched};

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "package_names",
        group: Group::Recommended,
        problem: "The package name '{0}' isn't a lower_case_with_underscores identifier.",
        correction: "Try changing the name to follow the lower_case_with_underscores style.",
        kinds: &[],
        check: not_dispatched,
    };

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

    use super::{Group, NodeKind, RuleSpec};

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "depend_on_referenced_packages",
        group: Group::Core,
        problem: "The imported package '{0}' isn't a dependency of the importing package.",
        correction: "Try adding a dependency for '{0}' in the 'pubspec.yaml' file.",
        kinds: &[NodeKind::Import, NodeKind::Export],
        check,
    };

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
