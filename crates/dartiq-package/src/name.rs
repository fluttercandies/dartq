//! Canonical package names (facts: `pubspec_parse.dart:17-19`).

use std::borrow::Borrow;

use crate::PackageError;

/// A package name usable as a map key, validated against its layer's
/// grammar.
///
/// The grammars are deliberately layered: pubspec-produced names (graph
/// root, workspace members) follow the consumption-side pub grammar
/// (`pubspec_parse.dart:17-19`, dot-separated Dart identifiers via
/// [`PackageName::new`]), while `package_config.json` entry names only
/// follow the reader's looser charset (see
/// `table::is_valid_config_name`) — the config file is consumed as pub
/// wrote it, not re-validated as if published. Both wrap this type; names
/// are matched exactly everywhere (no case folding; `pub.dev`'s stricter
/// lowercase publish rule does not apply to consuming a resolution).
///
/// `sdk:dart`-style instance names contain `:` and cannot be package names,
/// which keeps the implicit SDK instance out of name-keyed maps by type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

impl PackageName {
    /// Validates a name against pub's grammar.
    ///
    /// # Errors
    ///
    /// Returns [`PackageError::InvalidName`] unless the whole string is
    /// dot-separated identifiers.
    pub fn new(name: &str) -> Result<PackageName, PackageError> {
        if is_valid(name) {
            Ok(PackageName(name.to_owned()))
        } else {
            Err(PackageError::InvalidName {
                name: name.to_owned(),
            })
        }
    }

    /// Wraps a name the caller already validated against its layer's
    /// grammar — the table's config reader checks entries with the loose
    /// reader grammar before calling this. Crate-internal: public
    /// construction must go through [`PackageName::new`].
    pub(crate) fn new_unchecked(name: String) -> PackageName {
        PackageName(name)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for PackageName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `[a-zA-Z_][a-zA-Z0-9_]*` — pub's `identifierRegExp`
/// (`utils.dart:31`); `\w` there is exactly ASCII word characters.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `identifier(.identifier)*` — pub's `packageNameRegExp`
/// (`pubspec_parse.dart:17-19`).
fn is_valid(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(is_identifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(name: &str) -> bool {
        PackageName::new(name).is_ok()
    }

    #[test]
    fn accepts_pub_grammar_names() {
        for name in [
            "foo",
            "foo_bar",
            "_private",
            "a1_b2",
            "Foo",
            "foo.bar",
            "foo.bar_baz.qux",
        ] {
            assert!(valid(name), "`{name}` must be a valid package name");
        }
    }

    #[test]
    fn rejects_names_outside_the_grammar() {
        for name in [
            "", "1abc", "-foo", "foo-", "foo..bar", ".foo", "foo.", "foo bar", "foo/bar",
            "foo:bar", "sdk:dart", "föö",
        ] {
            assert!(!valid(name), "`{name}` must be rejected");
        }
    }

    #[test]
    fn invalid_names_carry_the_offender() {
        match PackageName::new("foo bar") {
            Err(PackageError::InvalidName { name }) => assert_eq!(name, "foo bar"),
            other => panic!("expected InvalidName, got {other:?}"),
        }
    }

    #[test]
    fn str_lookups_find_names_in_keyed_collections() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert(PackageName::new("hosted_dep").expect("valid"), 1);
        assert_eq!(map.get("hosted_dep"), Some(&1));
        assert_eq!(map.get("other"), None);
    }

    #[test]
    fn display_round_trips_the_name() {
        let name = PackageName::new("foo.bar_baz").expect("valid");
        assert_eq!(name.to_string(), "foo.bar_baz");
        assert_eq!(name.as_str(), "foo.bar_baz");
    }
}
