//! Parse-time validation of git dependency descriptions, mirroring pub's
//! `lib/src/source/git.dart`: the `tag_pattern` marker check
//! (`validateTagPattern`, git.dart:1315-1323), the URL shape check
//! (`_validatedUrl`, git.dart:200-239) and the path shape check
//! (`_validatedPath`, git.dart:250-277).
//!
//! Only the parse-time error faces are reproduced. Relative URLs always
//! resolve (dartq only parses local pubspecs, so the "isn't a local
//! pubspec" face is unreachable), and normalization of the stored values
//! is a resolution-time concern dartq has not reached.

/// A URI reference split the way Dart's lenient `Uri.parse` splits it
/// (sdk/lib/core/uri.dart `_Uri.parse`: pure component splitting, no
/// percent-encoding, IPv6 or port validation).
struct UriParts<'a> {
    /// Scheme text; `None` without a `scheme:` prefix (`:foo` is a path,
    /// not an empty scheme).
    scheme: Option<&'a str>,
    has_authority: bool,
    /// Text between the optional scheme/authority and the first `?`/`#`.
    path: &'a str,
    /// A `?` before any `#`.
    has_query: bool,
    has_fragment: bool,
}

impl<'a> UriParts<'a> {
    fn parse(s: &'a str) -> UriParts<'a> {
        let bytes = s.as_bytes();
        // scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":" — a
        // non-scheme start (digit, `:`, non-ASCII) puts the scanner on the
        // path, where `:` is a regular character.
        let mut rest = s;
        let mut scheme = None;
        if bytes.first().is_some_and(u8::is_ascii_alphabetic) {
            let end = bytes
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
                .count();
            if bytes.get(end) == Some(&b':') {
                scheme = Some(&s[..end]);
                rest = &s[end + 1..];
            }
        }
        // An authority is a `//` prefix before the path starts; a later
        // `//` is plain path text.
        let mut has_authority = false;
        if let Some(after) = rest.strip_prefix("//") {
            has_authority = true;
            let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
            rest = &after[authority_end..];
        }
        let query_end = rest.find('#').unwrap_or(rest.len());
        let path_end = rest[..query_end].find('?').unwrap_or(query_end);
        UriParts {
            scheme,
            has_authority,
            path: &rest[..path_end],
            has_query: path_end != query_end,
            has_fragment: query_end != rest.len(),
        }
    }
}

/// pub's `validateTagPattern` (git.dart:1315-1323): the pattern must hold
/// exactly one `{{version}}` marker; the surrounding text is matched
/// verbatim, there is no glob syntax to validate.
pub(crate) fn validate_tag_pattern(pattern: &str) -> Result<(), String> {
    const MARKER: &str = "{{version}}";
    if pattern.split(MARKER).count() == 2 {
        Ok(())
    } else {
        Err(
            "The `tag_pattern` must contain a single \"{{version}}\" to match different versions"
                .to_owned(),
        )
    }
}

/// pub's `GitSource._validatedUrl` (git.dart:200-239), parse-time faces:
/// no leading `-` (git option injection) and no `ext:` scheme. SCP-style
/// URLs (`user@host:repo`) are exempt from the URI check; relative URLs
/// always resolve against the containing pubspec's directory.
pub(crate) fn validate_url(url: &str) -> Result<(), String> {
    if url.starts_with('-') {
        return Err(format!("\"{url}\" is not a valid Git URL."));
    }
    let is_scp = match (url.find('@'), url.find(':')) {
        (Some(at), Some(colon)) => at < colon,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if !is_scp
        && UriParts::parse(url)
            .scheme
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ext"))
    {
        return Err(format!("\"{url}\" is not a valid Git URL."));
    }
    Ok(())
}

/// pub's `GitSource._validatedPath` (git.dart:250-277): the path must be a
/// relative URL path and must stay inside the repository.
pub(crate) fn validate_path(path: &str) -> Result<(), String> {
    let parts = UriParts::parse(path);
    if parts.scheme.is_some()
        || parts.has_authority
        || parts.path.starts_with('/')
        || parts.has_query
        || parts.has_fragment
    {
        return Err("The 'path' field of the description must be a relative path URL.".to_owned());
    }
    // `p.url.isWithin('.', path) || p.url.equals('.', path)` (git.dart:
    // 270-275) is normalization-based, so only paths whose normalization
    // keeps a leading `..` escape.
    let mut depth = 0u32;
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if depth == 0 {
                    return Err("The 'path' field of the description must not reach outside the repository.".to_owned());
                }
                depth -= 1;
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_parts_mirrors_dart_splitting() {
        let parts = UriParts::parse("https://user@host:8080/p?q#f");
        assert_eq!(parts.scheme, Some("https"));
        assert!(parts.has_authority);
        assert_eq!(parts.path, "/p");
        assert!(parts.has_query);
        assert!(parts.has_fragment);

        // A digit start is a path, `:` stays path text.
        let parts = UriParts::parse("1:foo");
        assert_eq!(parts.scheme, None);
        assert_eq!(parts.path, "1:foo");

        // An empty `:` prefix is not a scheme.
        assert_eq!(UriParts::parse(":foo").scheme, None);

        // `//` after the path started is path text, not an authority.
        let parts = UriParts::parse("a//b");
        assert_eq!(parts.scheme, None);
        assert!(!parts.has_authority);
        assert_eq!(parts.path, "a//b");

        // An authority ending at the string end leaves an empty path.
        let parts = UriParts::parse("//host");
        assert!(parts.has_authority);
        assert_eq!(parts.path, "");

        // A `?` inside a fragment does not open a query.
        let parts = UriParts::parse("a#b?c");
        assert!(!parts.has_query);
        assert!(parts.has_fragment);
    }

    #[test]
    fn tag_pattern_needs_exactly_one_marker() {
        assert!(validate_tag_pattern("v{{version}}").is_ok());
        assert!(validate_tag_pattern("release/v{{version}}+build").is_ok());
        assert_eq!(
            validate_tag_pattern("v100"),
            Err("The `tag_pattern` must contain a single \"{{version}}\" to match different versions".to_owned())
        );
        assert!(validate_tag_pattern("v{{version}}{{version}}").is_err());
    }

    #[test]
    fn url_rejects_option_injection_and_ext_scheme() {
        assert_eq!(
            validate_url("--upload-pack=evil"),
            Err("\"--upload-pack=evil\" is not a valid Git URL.".to_owned())
        );
        assert_eq!(
            validate_url("ext::sh -c touch /tmp/pwned @"),
            Err("\"ext::sh -c touch /tmp/pwned @\" is not a valid Git URL.".to_owned())
        );
        assert_eq!(
            validate_url("EXT://host/repo"),
            Err("\"EXT://host/repo\" is not a valid Git URL.".to_owned())
        );
        // SCP-style URLs skip the URI check entirely.
        assert!(validate_url("git@github.com:org/repo.git").is_ok());
        assert!(validate_url("user@host").is_ok());
        assert!(validate_url("https://github.com/org/repo.git").is_ok());
        assert!(validate_url("../local/repo").is_ok());
    }

    #[test]
    fn path_must_be_a_relative_url_inside_the_repository() {
        for bad in [
            "/subdir",
            "https://subdir",
            "subdir/dir#fragment",
            "subdir/dir?query",
            "bob:pwd@somewhere.example.com/subdir",
        ] {
            assert_eq!(
                validate_path(bad),
                Err("The 'path' field of the description must be a relative path URL.".to_owned()),
                "{bad}"
            );
        }
        for escape in ["../outside", "a/../../b", "a/../.."] {
            assert_eq!(
                validate_path(escape),
                Err(
                    "The 'path' field of the description must not reach outside the repository."
                        .to_owned()
                ),
                "{escape}"
            );
        }
        // Normalization-based containment: these all stay inside.
        for ok in [".", "", "sub/dir", "a/../b", "a/..", "sub/dir%25"] {
            assert!(validate_path(ok).is_ok(), "{ok}");
        }
    }
}
