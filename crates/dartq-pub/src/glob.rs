//! A pinned port of `package:glob` 2.1.3 (the version `.third/pub` pins),
//! restricted to what workspace member expansion needs: POSIX paths,
//! case-sensitive matching, symlink-following listing.
//!
//! Pub expands glob workspace entries with
//! `Glob(entry).listFileSystemSync(...)` rooted at the declaring pubspec's
//! directory (package.dart:183-208), so the observable semantics that matter
//! are the parser's (parser.dart), the option-flattened component split
//! (ast.dart `split`), and the list-tree walk (list_tree.dart): entries match
//! in readdir order, both files and directories are emitted, intermediate
//! directories that do not exist are silently skipped, and a `**` component
//! turns the walk into a pre-order recursive listing filtered by the
//! full-pattern match (`a/**/b` needs at least one intermediate segment —
//! the compiled regex `a/(?!^(?:\.\./|/))[^]*/b` cannot match `a/b`).
//!
//! Hand-written rather than a Rust wheel because the semantics follow the
//! official implementation: the `glob` crate has no `{a,b}` alternation (pub
//! treats `{` as a glob wildcard), and `globset` diverges on `**`
//! (zero-segment matches) and character-class syntax while still leaving the
//! filesystem traversal to us.

use std::path::{Path, PathBuf};

/// Whether `s` contains any glob wildcard character, per pub's detector
/// (package.dart:595-604 — backslash escapes are intentionally not honored).
pub(crate) fn has_glob_wildcards(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[') || s.contains('{')
}

/// A glob parse failure, rendered exactly like Dart's `StringScanner`
/// exceptions (`Error on line 1, column 8: expected ",".`), which pub
/// interpolates verbatim into its "Failed to parse glob" error
/// (package.dart:187-190).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobError {
    line: usize,
    column: usize,
    message: String,
}

impl std::fmt::Display for GlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error on line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

/// One glob AST node (`ast.dart`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Literal(String),
    /// `*` — zero or more non-separator characters.
    Star,
    /// `**` — zero or more characters including separators.
    DoubleStar,
    /// `?` — one non-separator character.
    AnyChar,
    /// `[...]` / `[!...]` / `[^...]` character ranges.
    Range {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    /// `{a,b}` — flattened away before matching/listing.
    Options(Vec<Sequence>),
}

/// A sequence of adjacent nodes (`ast.dart` `SequenceNode`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sequence(Vec<Node>);

/// The parsed glob: the list-tree root (`list_tree.dart` `ListTree`).
#[derive(Debug)]
pub(crate) struct Glob {
    root: TreeNode,
}

impl Glob {
    /// Parses a glob pattern (`glob.dart` `Glob.new`, defaults: relative,
    /// case-sensitive, non-recursive).
    pub(crate) fn new(pattern: &str) -> Result<Glob, GlobError> {
        let sequence = Parser::new(pattern).parse()?;
        // `ListTree`: flatten options (and singleton ranges) to top level,
        // then split each option into path components (`/` inside literals).
        let mut root = TreeNode::default();
        for option in flatten(sequence) {
            add_components(&mut root, &split(option));
        }
        Ok(Glob { root })
    }

    /// Lists every filesystem entity beneath `root_dir` matching the glob,
    /// in pub's listing order (readdir order per directory, options and
    /// children in declaration order), deduplicated by path when the tree
    /// can list one path twice — `listSync` guarantees one entity per path
    /// (glob.dart:113-116).
    ///
    /// Missing directories are silently skipped; any other I/O error
    /// surfaces here (package:glob only swallows the "missing" case,
    /// list_tree.dart:408-414, 429-435).
    pub(crate) fn list_sync(&self, root_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        self.root.list(root_dir, &mut out)?;
        // The seen-set of `ListTree.listSync` (list_tree.dart:209-215) only
        // filters when the same path can come out of two tree branches
        // (`_computeCanOverlap`, list_tree.dart:171-179); otherwise the
        // per-entry clone and insert is a pure no-op and is skipped.
        if self.root.can_overlap() {
            let mut seen = std::collections::BTreeSet::new();
            out.retain(|path| seen.insert(path.clone()));
        }
        Ok(out)
    }
}

/// A node in the list tree (`list_tree.dart` `_ListTreeNode`). `recursive`
/// mirrors Dart's `children == null`: the node is listed recursively and its
/// children folded into `validator`.
#[derive(Debug, Default)]
struct TreeNode {
    recursive: bool,
    /// Insertion-ordered child globs (Dart's `Map` preserves insertion).
    children: Vec<(Sequence, TreeNode)>,
    /// Validator options: entities whose (relative) path matches any of
    /// these are emitted.
    validator: Vec<Sequence>,
}

impl TreeNode {
    fn make_recursive(&mut self) {
        if self.recursive {
            return;
        }
        let children = std::mem::take(&mut self.children);
        // Dart replaces the validator with the folded children
        // (list_tree.dart:292-300) — pre-existing options are dropped, a
        // quirk of `{literal, **}`-shaped patterns this port preserves.
        let mut folded = Vec::new();
        for (sequence, mut child) in children {
            child.make_recursive();
            for option in child.validator {
                let mut joined = sequence.0.clone();
                joined.push(Node::Literal("/".to_owned()));
                joined.extend(option.0);
                folded.push(Sequence(joined));
            }
        }
        self.validator = folded;
        self.recursive = true;
    }

    fn list(&self, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if self.recursive {
            return self.list_recursive(dir, "", out);
        }
        // Don't list directories whose children are all literal names
        // (list_tree.dart:328-338); missing ones are silently skipped.
        if self.is_intermediate() {
            for (sequence, child) in &self.children {
                let Node::Literal(text) = &sequence.0[0] else {
                    unreachable!("intermediate children are single literals");
                };
                child.list(&dir.join(text), out)?;
            }
            return Ok(());
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let basename = entry.file_name().to_string_lossy().into_owned();
            if self
                .validator
                .iter()
                .any(|option| matches(&option.0, &basename))
            {
                out.push(path.clone());
            }
            // `entity is! Directory` with followLinks: a symlink to a
            // directory counts (list_tree.dart:355-361). dart:io reads the
            // type from the dirent and stats only symlinks; an
            // unresolvable entry counts as not-a-directory.
            let is_dir = match entry.file_type() {
                Ok(file_type) if file_type.is_symlink() => {
                    std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir())
                }
                Ok(file_type) => file_type.is_dir(),
                Err(_) => false,
            };
            if is_dir {
                for (sequence, child) in &self.children {
                    if matches(&sequence.0, &basename) {
                        child.list(&path, out)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `Directory.listSync(recursive: true, followLinks: true)` filtered by
    /// the validator, pre-order: each entity is emitted (if it matches)
    /// before its contents are walked (list_tree.dart:404-414).
    fn list_recursive(
        &self,
        dir: &Path,
        relative: &str,
        out: &mut Vec<PathBuf>,
    ) -> std::io::Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // Only "missing" is ever swallowed — pub's `isMissing` catches
            // errno 2/3 alone and rethrows the rest (list_tree.dart:404-414,
            // every failed directory open surfaces as kListError and is
            // thrown, directory.cc:543-551) — so a file (`ENOTDIR`) or an
            // unreadable directory surfaces here. A directory that goes
            // missing mid-walk is likewise not a hard error.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            if self
                .validator
                .iter()
                .any(|option| matches(&option.0, &child_relative))
            {
                out.push(path.clone());
            }
            // followLinks: dart:io takes the type straight from the dirent
            // and stats only symlinks (directory_linux.cc `Next`, DT_LNK
            // branch); a symlink to a directory counts (list_tree.dart:355-
            // 361), and an unresolvable entry counts as not-a-directory.
            let is_dir = match entry.file_type() {
                Ok(file_type) if file_type.is_symlink() => {
                    std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir())
                }
                Ok(file_type) => file_type.is_dir(),
                Err(_) => false,
            };
            if is_dir {
                let nested = format!("{child_relative}/");
                self.list_recursive(&path, &nested, out)?;
            }
        }
        Ok(())
    }

    fn is_intermediate(&self) -> bool {
        self.validator.is_empty()
            && self.children.iter().all(|(sequence, _)| {
                sequence.0.len() == 1 && matches!(&sequence.0[0], Node::Literal(_))
            })
    }

    /// Whether listing this node can return the same path twice
    /// (list_tree.dart:256-277). A recursive node is a single listing
    /// filtered by one validator and never overlaps; with more than one
    /// child, any dynamic child may match a sibling's match; a single child
    /// delegates. Case-insensitivity is out of scope: this port is
    /// case-sensitive only.
    fn can_overlap(&self) -> bool {
        if self.recursive {
            return false;
        }
        if self.children.len() > 1
            && self.children.iter().any(|(sequence, _)| {
                sequence.0.len() > 1 || !matches!(&sequence.0[0], Node::Literal(_))
            })
        {
            return true;
        }
        self.children.iter().any(|(_, node)| node.can_overlap())
    }
}

/// Adds one option's component chain to the tree (`list_tree.dart`
/// `_addGlob`; the root always exists here, so Dart's `parent == null`
/// branches collapse into the shared path).
fn add_components(root: &mut TreeNode, components: &[Sequence]) {
    let mut current = root;
    for (index, component) in components.iter().enumerate() {
        let recursive = component
            .0
            .iter()
            .any(|node| matches!(node, Node::DoubleStar));
        if recursive {
            current.make_recursive();
            current.validator.push(join(&components[index..]));
            return;
        }
        if index == components.len() - 1 {
            current.validator.push(component.clone());
            return;
        }
        let position = match current
            .children
            .iter()
            .position(|(seq, _)| seq == component)
        {
            Some(position) => position,
            None => {
                current
                    .children
                    .push((component.clone(), TreeNode::default()));
                current.children.len() - 1
            }
        };
        current = &mut current.children[position].1;
    }
}

/// Joins components with `/` separators (`list_tree.dart` `_join`).
fn join(components: &[Sequence]) -> Sequence {
    let mut nodes = components[0].0.clone();
    for component in &components[1..] {
        nodes.push(Node::Literal("/".to_owned()));
        nodes.extend(component.0.iter().cloned());
    }
    Sequence(nodes)
}

/// Splits a sequence into path components on the `/` inside literal nodes
/// (`ast.dart` `SequenceNode.split`, POSIX style). The split goes through
/// `p.context.split`, which drops empty parts (`foo/` is one component,
/// `a//b` two), so a trailing separator only forces a component boundary
/// before whatever follows. A literal starting with `/` parses as the
/// absolute root plus the rest (context.dart:364-370 inserts the root at
/// position 0), and the root branch of ast.dart:151-166 always finishes the
/// pending component — a hard boundary — so mid-pattern `/…` literals never
/// glue onto the component before them. The root literal itself is dropped:
/// workspace entries are always relative, so no tree is ever rooted there.
fn split(sequence: Sequence) -> Vec<Sequence> {
    let mut components: Vec<Sequence> = Vec::new();
    let mut current: Option<Vec<Node>> = None;
    fn flush(current: &mut Option<Vec<Node>>, components: &mut Vec<Sequence>) {
        if let Some(nodes) = current.take() {
            components.push(Sequence(nodes));
        }
    }
    for node in sequence.0 {
        match node {
            Node::Literal(text) if text.contains('/') => {
                // ast.dart:164 `finishComponent()` fires whether or not the
                // root literal is kept (ast.dart:153-163).
                if text.starts_with('/') {
                    flush(&mut current, &mut components);
                }
                let parts: Vec<&str> = text.split('/').filter(|part| !part.is_empty()).collect();
                let Some((last, head)) = parts.split_last() else {
                    continue; // `/`-only literal: boundary flushed above
                };
                for part in head {
                    current
                        .get_or_insert_with(Vec::new)
                        .push(Node::Literal((*part).to_owned()));
                    flush(&mut current, &mut components);
                }
                current
                    .get_or_insert_with(Vec::new)
                    .push(Node::Literal((*last).to_owned()));
                if text.ends_with('/') {
                    flush(&mut current, &mut components);
                }
            }
            other => current.get_or_insert_with(Vec::new).push(other),
        }
    }
    flush(&mut current, &mut components);
    components
}

/// Bubbles every `Options` node (and non-negated singleton ranges) up to a
/// list of option-free sequences (`ast.dart` `flattenOptions`).
fn flatten(sequence: Sequence) -> Vec<Sequence> {
    let Sequence(nodes) = sequence;
    if nodes.is_empty() {
        return vec![Sequence(nodes)];
    }
    let mut sequences = flatten_node(nodes[0].clone());
    for node in nodes.into_iter().skip(1) {
        let next = flatten_node(node);
        sequences = sequences
            .into_iter()
            .flat_map(|prefix| {
                next.iter().map(move |suffix| {
                    Sequence(
                        prefix
                            .0
                            .iter()
                            .cloned()
                            .chain(suffix.0.iter().cloned())
                            .collect(),
                    )
                })
            })
            .collect();
    }
    sequences
}

fn flatten_node(node: Node) -> Vec<Sequence> {
    match node {
        Node::Options(options) => options.into_iter().flat_map(flatten).collect(),
        // A range explicitly listing single characters expands to literals
        // (ast.dart:297-310), which is what puts its alternatives into the
        // component tree in declaration order.
        Node::Range {
            negated: false,
            ranges,
        } if ranges.iter().all(|(lo, hi)| lo == hi) => ranges
            .iter()
            .map(|(lo, _)| Sequence(vec![Node::Literal(lo.to_string())]))
            .collect(),
        other => vec![Sequence(vec![other])],
    }
}

/// Backtracking full match of an option-free sequence against `text`.
///
/// The `DoubleStar` negative lookahead of `ast.dart:229-252` (`../` or `/`
/// prefixes) is elided: validators only ever see paths produced by listing
/// under a root, which never carry those prefixes.
fn matches(nodes: &[Node], text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    matches_sequence(nodes, &chars)
}

fn matches_sequence(nodes: &[Node], text: &[char]) -> bool {
    let Some((first, rest)) = nodes.split_first() else {
        return text.is_empty();
    };
    match first {
        Node::Literal(literal) => {
            let literal: Vec<char> = literal.chars().collect();
            text.len() >= literal.len()
                && text[..literal.len()] == literal[..]
                && matches_sequence(rest, &text[literal.len()..])
        }
        Node::Star => (0..=text.len())
            .take_while(|&index| index == 0 || text[index - 1] != '/')
            .any(|index| matches_sequence(rest, &text[index..])),
        Node::DoubleStar => (0..=text.len()).any(|index| matches_sequence(rest, &text[index..])),
        Node::AnyChar => !text.is_empty() && text[0] != '/' && matches_sequence(rest, &text[1..]),
        Node::Range { negated, ranges } => {
            !text.is_empty()
                && text[0] != '/'
                && ranges
                    .iter()
                    .any(|(lo, hi)| *lo <= text[0] && text[0] <= *hi)
                    != *negated
                && matches_sequence(rest, &text[1..])
        }
        Node::Options(_) => unreachable!("options are flattened before matching"),
    }
}

/// A `StringScanner` over chars (parser.dart uses UTF-16 offsets; the
/// line/column reported here only differ for astral-plane pattern
/// characters, which are not a concern for path entries).
struct Parser {
    chars: Vec<char>,
    position: usize,
}

impl Parser {
    fn new(pattern: &str) -> Parser {
        Parser {
            chars: pattern.chars().collect(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<Sequence, GlobError> {
        self.parse_sequence(false)
    }

    fn error(&self, position: usize, message: &str) -> GlobError {
        let line = 1 + self.chars[..position]
            .iter()
            .filter(|&&c| c == '\n')
            .count();
        let line_start = self.chars[..position]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |index| index + 1);
        GlobError {
            line,
            column: position - line_start + 1,
            message: message.to_owned(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn scan(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn fail(&self, expected: &str) -> GlobError {
        self.error(self.position, &format!("expected {expected}."))
    }

    fn read_char(&mut self) -> Result<char, GlobError> {
        match self.peek() {
            Some(c) => {
                self.position += 1;
                Ok(c)
            }
            None => Err(self.fail("more input")),
        }
    }

    fn parse_sequence(&mut self, in_options: bool) -> Result<Sequence, GlobError> {
        if self.position == self.chars.len() {
            return Err(self.error(0, "expected a glob."));
        }
        let mut nodes = Vec::new();
        while self.position < self.chars.len() {
            if in_options && (self.peek() == Some(',') || self.peek() == Some('}')) {
                break;
            }
            nodes.push(self.parse_node(in_options)?);
        }
        Ok(Sequence(nodes))
    }

    fn parse_node(&mut self, in_options: bool) -> Result<Node, GlobError> {
        if self.scan('*') {
            return Ok(if self.scan('*') {
                Node::DoubleStar
            } else {
                Node::Star
            });
        }
        if self.scan('?') {
            return Ok(Node::AnyChar);
        }
        if self.peek() == Some('[') {
            return self.parse_range();
        }
        if self.peek() == Some('{') {
            return self.parse_options();
        }
        self.parse_literal(in_options)
    }

    fn parse_range(&mut self) -> Result<Node, GlobError> {
        self.position += 1; // '['
        if self.peek() == Some(']') {
            return Err(self.error(self.position, "unexpected \"]\"."));
        }
        let negated = self.scan('!') || self.scan('^');
        let mut ranges = Vec::new();
        // Allows a backslash to escape a character; `/` may not appear.
        let read_range_char = |parser: &mut Parser| -> Result<char, GlobError> {
            let c = parser.read_char()?;
            if negated || c != '/' {
                Ok(c)
            } else {
                Err(parser.error(parser.position - 1, "\"/\" may not be used in a range."))
            }
        };
        while !self.scan(']') {
            let start = self.position;
            self.scan('\\');
            let lo = read_range_char(self)?;
            if self.scan('-') {
                if self.peek() == Some(']') {
                    ranges.push((lo, lo));
                    ranges.push(('-', '-'));
                    continue;
                }
                self.scan('\\');
                let hi = read_range_char(self)?;
                if hi < lo {
                    return Err(self.error(start, "Range out of order."));
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        Ok(Node::Range { negated, ranges })
    }

    fn parse_options(&mut self) -> Result<Node, GlobError> {
        self.position += 1; // '{'
        if self.peek() == Some('}') {
            return Err(self.error(self.position, "unexpected \"}\"."));
        }
        let mut options = Vec::new();
        loop {
            options.push(self.parse_sequence(true)?);
            if !self.scan(',') {
                break;
            }
        }
        // Single-option blocks are not allowed.
        if options.len() == 1 {
            return Err(self.fail("\",\""));
        }
        if !self.scan('}') {
            return Err(self.fail("\"}\""));
        }
        Ok(Node::Options(options))
    }

    fn parse_literal(&mut self, in_options: bool) -> Result<Node, GlobError> {
        // Commas are only literals outside options blocks (parser.dart:154-166).
        let is_plain = |c: char| {
            if c == ',' {
                return !in_options;
            }
            !matches!(c, '*' | '{' | '[' | '?' | '\\' | '}' | ']' | '(' | ')')
        };
        let mut buffer: String = self.take_while(is_plain);
        while self.scan('\\') {
            buffer.push(self.read_char()?);
            buffer.push_str(&self.take_while(is_plain));
        }
        for unexpected in [']', '(', ')'] {
            if self.peek() == Some(unexpected) {
                return Err(self.error(self.position, &format!("unexpected \"{unexpected}\"")));
            }
        }
        if !in_options && self.peek() == Some('}') {
            return Err(self.error(self.position, "unexpected \"}\""));
        }
        Ok(Node::Literal(buffer))
    }

    fn take_while(&mut self, mut predicate: impl FnMut(char) -> bool) -> String {
        let start = self.position;
        while self.chars.get(self.position).is_some_and(|&c| predicate(c)) {
            self.position += 1;
        }
        self.chars[start..self.position].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(pattern: &str, root: &Path) -> Vec<String> {
        Glob::new(pattern)
            .expect("parse")
            .list_sync(root)
            .expect("list")
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .expect("below root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn matches(pattern: &str, text: &str) -> bool {
        let flattened = flatten(Parser::new(pattern).parse().expect("parse"));
        flattened
            .iter()
            .any(|option| super::matches(&option.0, text))
    }

    #[test]
    fn wildcard_detection_matches_pub() {
        assert!(has_glob_wildcards("pkgs/*"));
        assert!(has_glob_wildcards("pkgs/?x"));
        assert!(has_glob_wildcards("pkgs/[ab]"));
        assert!(has_glob_wildcards("pkgs/{a,b}"));
        assert!(!has_glob_wildcards("pkgs/plain"));
    }

    #[test]
    fn star_and_question_match_within_components() {
        assert!(matches("pkgs/*", "pkgs/a"));
        assert!(matches("pkgs/*", "pkgs/a_b-c"));
        assert!(!matches("pkgs/*", "pkgs/a/b"));
        assert!(!matches("pkgs/*", "pkgs"));
        assert!(matches("pkgs/?", "pkgs/a"));
        assert!(!matches("pkgs/??", "pkgs/a"));
        assert!(matches("a/*/c", "a/b/c"));
    }

    #[test]
    fn double_star_matches_across_segments_but_needs_one() {
        // `**` matches any number of characters including separators, but
        // `a/**/b` cannot match `a/b` (needs one intermediate segment).
        assert!(matches("a/**/b", "a/x/b"));
        assert!(matches("a/**/b", "a/x/y/b"));
        assert!(!matches("a/**/b", "a/b"));
        assert!(matches("a/**", "a/x"));
        assert!(matches("a/**", "a/x/y"));
        assert!(!matches("a/**", "a"));
        assert!(matches("**/b", "x/y/b"));
        assert!(!matches("**/b", "b"));
    }

    #[test]
    fn ranges_and_options_match() {
        assert!(matches("pkgs/[ab]", "pkgs/a"));
        assert!(!matches("pkgs/[ab]", "pkgs/c"));
        assert!(matches("pkgs/[a-c]", "pkgs/b"));
        assert!(matches("pkgs/[!a]", "pkgs/b"));
        assert!(!matches("pkgs/[!a]", "pkgs/a"));
        assert!(matches("pkgs/{a,b}", "pkgs/a"));
        assert!(matches("pkgs/{a,b}", "pkgs/b"));
        assert!(!matches("pkgs/{a,b}", "pkgs/c"));
        assert!(matches("pkgs/{a,?}/c", "pkgs/x/c"));
        // Escapes produce literal characters.
        assert!(matches(r"pkgs/\*word", "pkgs/*word"));
    }

    #[test]
    fn parse_errors_render_like_string_scanner() {
        let error = Glob::new("pkgs/{*").expect_err("unclosed options");
        assert_eq!(
            error.to_string(),
            "Error on line 1, column 8: expected \",\"."
        );
        let error = Glob::new("pkgs/}").expect_err("stray brace");
        assert_eq!(
            error.to_string(),
            "Error on line 1, column 6: unexpected \"}\""
        );
        let error = Glob::new("pkgs/[]").expect_err("empty range");
        assert_eq!(
            error.to_string(),
            "Error on line 1, column 7: unexpected \"]\"."
        );
        let error = Glob::new("pkgs/[b-a]").expect_err("out of order");
        assert_eq!(
            error.to_string(),
            "Error on line 1, column 7: Range out of order."
        );
        let error = Glob::new("pkgs/{a}").expect_err("single option");
        assert_eq!(
            error.to_string(),
            "Error on line 1, column 8: expected \",\"."
        );
    }

    #[test]
    fn lists_in_readdir_order_deduped() {
        let dir = std::env::temp_dir().join(format!("dartq-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["b", "a"] {
            std::fs::create_dir_all(dir.join("pkgs").join(name)).expect("mkdir");
            std::fs::write(dir.join("pkgs").join(name).join("lib.rs"), "").expect("write");
        }
        std::fs::write(dir.join("pkgs").join("note.txt"), "").expect("write");

        // readdir order, files and directories alike; package:glob makes no
        // order guarantee, so only membership is asserted.
        let found = listed("pkgs/*", &dir);
        assert_eq!(found.len(), 3);
        assert!(found.contains(&"pkgs/a".to_owned()));
        assert!(found.contains(&"pkgs/b".to_owned()));
        assert!(found.contains(&"pkgs/note.txt".to_owned()));

        // Same-level options merge into one validator: readdir order again.
        let mut found = listed("pkgs/{b,a}", &dir);
        found.sort();
        assert_eq!(found, vec!["pkgs/a", "pkgs/b"]);

        // Distinct literal prefixes become separate tree branches, walked in
        // declaration order.
        let mut found = listed("{pkgs/a,pkgs}", &dir);
        found.sort();
        assert_eq!(found, vec!["pkgs", "pkgs/a"]);

        // Missing literal intermediate directories are silently skipped.
        assert!(listed("nope/*", &dir).is_empty());
        assert!(listed("pkgs/missing/*", &dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recursive_listing_walks_preorder_filtered() {
        let dir = std::env::temp_dir().join(format!("dartq-glob-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a").join("inner")).expect("mkdir");
        std::fs::write(dir.join("a").join("pkg.dart"), "").expect("write");
        std::fs::write(dir.join("a").join("inner").join("deep.dart"), "").expect("write");
        assert_eq!(listed("a/**/deep.dart", &dir), vec!["a/inner/deep.dart"]);
        let found = listed("a/**", &dir);
        assert!(found.contains(&"a/pkg.dart".to_owned()));
        assert!(found.contains(&"a/inner/deep.dart".to_owned()));
        assert!(!found.contains(&"a".to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leading_slash_literals_flush_component_boundaries() {
        // Mid-pattern `/…` literals go through the absolute-root branch of
        // ast.dart:151-166, whose `finishComponent()` always closes the
        // pending component. Without that boundary `pkgs/{a,b}/lib` glues
        // into a validator matching the basenames `alib`/`blib`,
        // `packages/*/lib` glues `*` and `lib` together, `**/b` becomes one
        // component matching any path ending in `b`, and `pkgs/{a,b}/**`
        // folds `**` into a superset `[a, **]` validator.
        let dir = std::env::temp_dir().join(format!("dartq-glob-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["a", "b"] {
            std::fs::create_dir_all(dir.join("pkgs").join(name).join("lib")).expect("mkdir");
        }
        std::fs::create_dir_all(dir.join("pkgs").join("ab")).expect("mkdir");
        std::fs::write(dir.join("pkgs").join("ab").join("z.dart"), "").expect("write");
        std::fs::write(dir.join("pkgs").join("alib"), "").expect("write");
        std::fs::write(dir.join("pkgs").join("blib"), "").expect("write");
        std::fs::create_dir_all(dir.join("packages").join("a").join("lib")).expect("mkdir");
        std::fs::create_dir_all(dir.join("packages").join("b").join("lib")).expect("mkdir");
        std::fs::create_dir_all(dir.join("packages").join("lib")).expect("mkdir");
        std::fs::create_dir_all(dir.join("x")).expect("mkdir");
        std::fs::write(dir.join("b"), "").expect("write");
        std::fs::write(dir.join("x").join("b"), "").expect("write");

        // `pkgs/{a,b}/lib` descends into both members in declaration order
        // instead of matching the glued basenames.
        assert_eq!(
            listed("pkgs/{a,b}/lib", &dir),
            vec!["pkgs/a/lib", "pkgs/b/lib"]
        );

        // `packages/*/lib` lists the member lib dirs, not a glued
        // `…lib` basename under packages (readdir order, hence sorted).
        let mut found = listed("packages/*/lib", &dir);
        found.sort();
        assert_eq!(found, vec!["packages/a/lib", "packages/b/lib"]);

        // `**/b` needs at least one intermediate segment, consistent with
        // the raw matcher (`!matches("**/b", "b")`).
        assert_eq!(listed("**/b", &dir), vec!["x/b"]);

        // `pkgs/{a,b}/**` recurses under both members only; `pkgs/ab/**`
        // would match a glued `[a, **]` validator.
        let mut found = listed("pkgs/{a,b}/**", &dir);
        found.sort();
        assert_eq!(found, vec!["pkgs/a/lib", "pkgs/b/lib"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recursive_listing_rethrows_non_missing_top_level_errors() {
        // pub's `isMissing` catches errno 2/3 only (list_tree.dart:404-414):
        // `a/**` over a file rethrows `ENOTDIR` instead of yielding an empty
        // list, while a missing top-level directory stays swallowed.
        let dir = std::env::temp_dir().join(format!("dartq-glob-enotdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("a"), "").expect("write");
        assert!(Glob::new("a/**").expect("parse").list_sync(&dir).is_err());
        assert!(listed("missing/**", &dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
