//! Package-name grammar boundary pins (facts: `pubspec_parse.dart:17-19`
//! with `identifierRegExp = [a-zA-Z_]\w*`, `utils.dart:31`): dot-separated
//! identifiers where every segment starts with a letter or underscore.

use dartiq_package::PackageName;

#[test]
fn numeric_dot_segments_are_rejected() {
    // `foo.1`'s second segment starts with a digit, so it is not an
    // identifier under pub's `packageNameRegExp` — the dots widen the
    // grammar for Google-internal dotted packages only, never to
    // non-identifier segments.
    assert!(PackageName::new("foo.1").is_err());
    assert!(PackageName::new("1").is_err());
    assert!(PackageName::new("foo.1bar").is_err());
}

#[test]
fn identifier_dot_segments_are_accepted() {
    // Contrast pin: a segment starting with an underscore is an identifier.
    assert!(PackageName::new("foo.bar").is_ok());
    assert!(PackageName::new("foo._1").is_ok());
}
