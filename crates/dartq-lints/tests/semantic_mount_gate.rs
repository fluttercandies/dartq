//! The semantic-mount gate: every registered rule whose check call-graph
//! reads [`Check::semantics`] must be named in `SEMANTIC_RULES` — the
//! `mounts_semantics` predicate is the only thing that mounts the
//! per-file `Analysis` fold in production, so a consumer missing from the
//! table runs with `c.semantics() == None` forever and every row it would
//! emit is silently dropped (the #3527 / #3530 silent-death class:
//! `discarded_futures`, `unawaited_futures`, `prefer_const_constructors`).
//!
//! The pin this adds over `tests/registry_pin.rs`'s
//! `semantic_rules_membership`: that pin holds the table's
//! self-consistency — it cannot see a rule that reads the fold but was
//! never registered. This gate derives the consumer set from the source
//! tree itself, so a new rule (or a new fold read inside an existing
//! rule) fails immediately, naming the offending file and rule — no
//! cooperation from the rule's author required.
//!
//! Method: a lightweight source scan, not a compiler. Each `src/rules`
//! file is stripped of comments and literals, its items are indexed
//! (inner `mod`s, `fn`s, `spec!`/`RuleSpec` declarations), calls are
//! resolved by scope (same mod, same file, qualified path, crate-global),
//! and a fixed point marks every fn reachable to a `.semantics()` read.
//! A rule whose `check` reads is a fold consumer; it must appear in
//! `SEMANTIC_RULES` or carry an explicit, reasoned [`ALLOWLIST`] entry.
//! The scan is conservative by design — over-attribution only ever
//! demands a registration (a stale name costs fold-mount work, never
//! correctness), while under-attribution would reproduce the silence
//! this gate exists to kill.
//!
//! The separate mount seams (`Unit::async_context_uses`,
//! `Unit::reachability`, `Unit::library_type_uses`) are keyed by rule
//! name inside the driver (`crates/dartq-commands/src/lints.rs`), not
//! by `SEMANTIC_RULES`; [`every_driver_seam_gate_matches_the_registry_and_the_face_readers`]
//! below runs the same source-walk against that driver — every
//! `rule_by_name("...")` gate it names must be a live registered rule,
//! and every rule a scan proves reads one of the mounted faces must be
//! one of the names it gates.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use dartq_lints::{Group, SEMANTIC_RULES, rules};

/// Rules that read `Check::semantics()` yet legitimately stay out of
/// `SEMANTIC_RULES`, each with the reason the absence is correct. The
/// escape hatch exists for a read that is a genuine optional refinement —
/// never for a read the rule's verdicts depend on (that is the death
/// class). Empty today: every current reader belongs to the table.
const ALLOWLIST: &[(&str, &str)] = &[];

/// One indexed `fn` item: its name, the module it sits in, and the
/// byte range of its brace body inside the stripped source.
struct FnItem {
    module: usize,
    name: String,
    body: (usize, usize),
}

/// One rule declaration: the registered name, the `check` function's
/// name, and the module the declaration sits in.
struct SpecItem {
    module: usize,
    name: String,
    check: String,
}

/// A parsed source file: every `mod`/`fn`/rule item it declares.
struct SourceFile {
    /// Repo-relative path, for failure messages (`src/rules/...`).
    path: String,
    /// The module this file IS from its parent's view (the file stem —
    /// `unused_futures`, `const_ability`). Index 0 of `modules`.
    stem: String,
    /// Module names declared in the file: entry 0 is the file itself,
    /// deeper entries are inner `mod`s named `parent::name`.
    modules: Vec<String>,
    fns: Vec<FnItem>,
    specs: Vec<SpecItem>,
}

/// The whole scan: files, a global fn-name index, and the fixed-point
/// read set the check-fns are judged against.
struct Scan {
    files: Vec<SourceFile>,
    /// Fn name → every fn item carrying it.
    by_name: HashMap<String, Vec<(usize, usize)>>,
    /// reads_fold[file][fn] — the fixed-point result.
    reads: Vec<Vec<bool>>,
    /// The call graph the fixed point ran over — kept so a failure can
    /// name the propagation path back to the direct `.semantics()` read.
    edges: Vec<Vec<BTreeSet<(usize, usize)>>>,
}

/// Walk `src/rules` for `.rs` files, skipping pin-test modules — the
/// `*_pins` convention never hosts a registered rule's check, and its own
/// reads (fixture mounts, `run_with_semantics`) are not production
/// consumption. A rule name may legitimately contain `test`
/// (`test_types_in_equals`, `use_test_throws_matchers`), so the stem is
/// matched on the pin suffix only, never on a substring.
fn rule_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}")) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rule_files(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.ends_with("_pins") {
            continue;
        }
        out.push(path);
    }
}

/// Whether `bytes[i]` starts an identifier char.
fn ident_at(bytes: &[u8], i: usize) -> bool {
    i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
}

/// Whether `bytes[i]` continues an identifier.
fn ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Blank comments and literals to spaces, preserving byte offsets (the
/// positions stay meaningful for line-number reporting). Line `//` and
/// block `/* */` comments, `"..."` and `r#"..."#` strings, and char
/// literals (`'x'`) are noise: a doc comment mentioning `c.semantics()`
/// must never register as a fold read (the codebase documents its bail
/// posture with exactly that spelling), and a string can carry braces.
fn strip_noise(src: &str) -> Vec<u8> {
    let bytes = src.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
        } else if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    depth -= 1;
                } else if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    depth += 1;
                } else {
                    if bytes[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
        } else if bytes[i] == b'r' && i + 1 < bytes.len() && bytes[i + 1] == b'#' {
            // Raw string `r#"..."#` (any hash count).
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] == b'#' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                let hashes = j - (i + 1);
                for k in i..=j {
                    out[k] = b' ';
                }
                i = j + 1;
                while i < bytes.len() {
                    if bytes[i] == b'"'
                        && i + hashes < bytes.len()
                        && bytes[i + 1..=i + hashes].iter().all(|&b| b == b'#')
                    {
                        for k in i..=i + hashes {
                            out[k] = b' ';
                        }
                        i += hashes + 1;
                        break;
                    }
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else if bytes[i] == b'r' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            if i < bytes.len() {
                out[i] = b' ';
                i += 1;
            }
        } else if bytes[i] == b'"' {
            out[i] = b' ';
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                } else {
                    if bytes[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
            if i < bytes.len() {
                out[i] = b' ';
                i += 1;
            }
        } else if bytes[i] == b'\'' {
            // A char literal `'x'`/`'\n'`/`'\u{..}'` — a lifetime `'a`
            // never closes within a few bytes, so probe for a closing
            // quote before blanking.
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'\\' {
                j += 2;
                while j < bytes.len() && bytes[j] != b'\'' && bytes[j] != b'\n' {
                    j += 1;
                }
            } else {
                while j < bytes.len() && bytes[j] != b'\'' && bytes[j] != b'\n' {
                    j += 1;
                }
            }
            if j < bytes.len() && bytes[j] == b'\'' && j - i <= 8 {
                for k in i..=j {
                    out[k] = b' ';
                }
                i = j + 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The byte position just past the balanced `open`-delimiter block
/// starting at `from` (which indexes the opening delimiter). Braces and
/// parens only — the stripped text carries no literal braces.
fn balanced(text: &[u8], from: usize, open: u8, close: u8) -> usize {
    let mut depth = 0usize;
    let mut i = from;
    while i < text.len() {
        if text[i] == open {
            depth += 1;
        } else if text[i] == close {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    text.len()
}

/// The identifier ending at `end` (exclusive), walking backward over
/// ident chars. Returns its start offset.
fn ident_before(text: &[u8], mut end: usize) -> usize {
    while end > 0 && ident_continue(text[end - 1]) {
        end -= 1;
    }
    end
}

/// The identifier starting at `start`, walking forward.
fn ident_after(text: &[u8], mut start: usize) -> usize {
    while start < text.len() && ident_continue(text[start]) {
        start += 1;
    }
    start
}

/// Whether the keyword `kw` occurs at `pos` with non-ident boundaries.
fn keyword_at(text: &[u8], pos: usize, kw: &str) -> bool {
    let kw = kw.as_bytes();
    pos + kw.len() <= text.len()
        && &text[pos..pos + kw.len()] == kw
        && (pos == 0 || !ident_continue(text[pos - 1]))
        && !ident_at(text, pos + kw.len())
}

/// Parse one source file into its module/fn/spec index.
fn parse_file(path: &Path, rel: &str) -> SourceFile {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
    let text = strip_noise(&src);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_owned();

    let mut file = SourceFile {
        path: rel.to_owned(),
        stem: stem.clone(),
        modules: vec![stem],
        fns: Vec::new(),
        specs: Vec::new(),
    };

    // The item walk records, per byte position, which module encloses it:
    // mod_ranges[module] = (body_start, body_end). Module 0 is the file.
    let mut mod_ranges: Vec<(usize, usize)> = vec![(0, text.len())];
    // (start, end, module_index) pairs for inner mods, so a fn/spec can
    // find its innermost enclosing module.
    let mut inner_mods: Vec<(usize, usize, usize)> = Vec::new();

    let mut i = 0;
    while i < text.len() {
        // `mod name {` — inner mods only (file-level `mod x;` declares
        // nothing here; the file itself is the module).
        if keyword_at(&text, i, "mod") {
            let mut j = i + 3;
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            let name_end = ident_after(&text, j);
            if name_end > j {
                let name = &src[j..name_end];
                let mut k = name_end;
                while k < text.len() && text[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < text.len() && text[k] == b'{' {
                    // A test module's reads are not production reads:
                    // `mod tests`/`mod test` by convention, plus any
                    // `#[cfg(test)]`-gated body.
                    let cfg_test = text[i.saturating_sub(64)..i]
                        .windows(b"cfg(test)".len())
                        .any(|w| w == b"cfg(test)");
                    if name != "tests" && name != "test" && !cfg_test {
                        // The enclosing module is the innermost range
                        // holding this `mod` keyword.
                        let parent = innermost(&mod_ranges, &inner_mods, i);
                        let qualified = format!("{}::{name}", file.modules[parent]);
                        file.modules.push(qualified);
                        let module = file.modules.len() - 1;
                        let end = balanced(&text, k, b'{', b'}');
                        mod_ranges.push((k + 1, end.saturating_sub(1)));
                        inner_mods.push((i, end, module));
                        // Keep walking INSIDE the mod body — its items
                        // are indexed against the new module scope.
                        i = k + 1;
                        continue;
                    }
                    i = balanced(&text, k, b'{', b'}');
                    continue;
                }
            }
            i += 3;
            continue;
        }
        // `fn name(` — the body is the next `{..}` block.
        if keyword_at(&text, i, "fn") {
            let mut j = i + 2;
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            let name_end = ident_after(&text, j);
            if name_end > j {
                let name = src[j..name_end].to_owned();
                // Find the body brace past the signature (generics,
                // args, where clause) — the first `{` before `;`.
                let mut k = name_end;
                let mut semi = false;
                while k < text.len() && text[k] != b'{' && text[k] != b';' {
                    k += 1;
                }
                if k < text.len() && text[k] == b';' {
                    semi = true;
                }
                if !semi && k < text.len() {
                    let end = balanced(&text, k, b'{', b'}');
                    let module = innermost(&mod_ranges, &inner_mods, i);
                    file.fns.push(FnItem {
                        module,
                        name,
                        body: (k + 1, end.saturating_sub(1)),
                    });
                    i = end;
                    continue;
                }
            }
            i += 2;
            continue;
        }
        // `spec! (` — the macro form. Name = first string literal inside
        // the parens (read from the source — the name is the first arg);
        // check fn = last ident of the stripped args.
        if keyword_at(&text, i, "spec") && i + 4 < text.len() && text[i + 4] == b'!' {
            let mut j = i + 5;
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < text.len() && text[j] == b'(' {
                let end = balanced(&text, j, b'(', b')');
                let stop = end.saturating_sub(1).max(j + 1);
                if let Some((name, check)) = spec_args(&src[j + 1..stop], &text[j + 1..stop]) {
                    let module = innermost(&mod_ranges, &inner_mods, i);
                    file.specs.push(SpecItem {
                        module,
                        name,
                        check,
                    });
                }
                i = end;
                continue;
            }
        }
        // `RuleSpec {` — the literal form. Fields `name:`/`check:` are
        // located in the stripped text (prose inside the message strings
        // cannot shadow them) and read back from the source.
        if keyword_at(&text, i, "RuleSpec") {
            let mut j = i + "RuleSpec".len();
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < text.len() && text[j] == b'{' {
                let end = balanced(&text, j, b'{', b'}');
                let stop = end.saturating_sub(1).max(j + 1);
                if let Some((name, check)) = literal_fields(&src[j + 1..stop], &text[j + 1..stop]) {
                    let module = innermost(&mod_ranges, &inner_mods, i);
                    file.specs.push(SpecItem {
                        module,
                        name,
                        check,
                    });
                }
                i = end;
                continue;
            }
        }
        // `removed_rule!(name, "name", ...)` — the removed-registry
        // generator; its check is `crate::rules::not_dispatched`.
        if keyword_at(&text, i, "removed_rule") && i + 12 < text.len() && text[i + 12] == b'!' {
            let mut j = i + 13;
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < text.len() && text[j] == b'(' {
                let end = balanced(&text, j, b'(', b')');
                let inner = &src[j + 1..end.saturating_sub(1).max(j + 1)];
                // The first string literal is the registered name.
                if let Some(start) = inner.find('"') {
                    if let Some(stop) = inner[start + 1..].find('"') {
                        let name = inner[start + 1..start + 1 + stop].to_owned();
                        let module = innermost(&mod_ranges, &inner_mods, i);
                        file.specs.push(SpecItem {
                            module,
                            name,
                            check: "not_dispatched".to_owned(),
                        });
                    }
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    file
}

/// The innermost module index enclosing `pos`: the last declared inner
/// mod whose body range holds it, else the file (0).
fn innermost(
    mod_ranges: &[(usize, usize)],
    inner_mods: &[(usize, usize, usize)],
    pos: usize,
) -> usize {
    let mut module = 0;
    let mut best = usize::MAX;
    for &(start, end, m) in inner_mods {
        let (body_start, body_end) = mod_ranges[m];
        if pos >= start && pos < end && body_end - body_start < best {
            best = body_end - body_start;
            module = m;
        }
    }
    module
}

/// The `spec!(name, kinds, check)` args: the first string literal is
/// the rule name (read on the source slice — the name is the first arg);
/// the last ident of the stripped args is the check fn (the arg is
/// `check` or `check,` — field-init shorthand carries the same name).
fn spec_args(inner: &str, stripped: &[u8]) -> Option<(String, String)> {
    let name_start = inner.find('"')?;
    let name_end = inner[name_start + 1..].find('"')? + name_start + 1;
    let name = inner[name_start + 1..name_end].to_owned();
    let mut end = stripped.len();
    while end > 0 && !ident_continue(stripped[end - 1]) {
        end -= 1;
    }
    let start = ident_before(stripped, end);
    if start == end {
        return None;
    }
    Some((name, inner[start..end].to_owned()))
}

/// The `name:`/`check:` fields of a `RuleSpec { .. }` literal: field
/// labels are found in the stripped text (so prose inside the message
/// strings cannot produce a fake `name:`), values read on the source.
fn literal_fields(inner: &str, stripped: &[u8]) -> Option<(String, String)> {
    // The offset just past `<key>:` for the first word-boundaried
    // occurrence, or None.
    let field_pos = |key: &str| -> Option<usize> {
        let key = key.as_bytes();
        let mut at = 0;
        while at + key.len() <= stripped.len() {
            if &stripped[at..at + key.len()] == key
                && (at == 0 || !ident_continue(stripped[at - 1]))
                && !ident_at(stripped, at + key.len())
            {
                let mut i = at + key.len();
                while i < stripped.len() && stripped[i] != b':' && stripped[i] != b',' {
                    i += 1;
                }
                if i < stripped.len() && stripped[i] == b':' {
                    return Some(i + 1);
                }
            }
            at += 1;
        }
        None
    };
    let name_pos = field_pos("name")?;
    // The name value is the first string literal after `name:` — a
    // macro variable (`$name`, the removed-registry template) is not a
    // rule name and the whole item is skipped.
    let name_start = inner[name_pos..].find('"')? + name_pos;
    let name_end = inner[name_start + 1..].find('"')? + name_start + 1;
    let name = inner[name_start + 1..name_end].to_owned();
    if name.is_empty() {
        return None;
    }
    let check = match field_pos("check") {
        // `check: <expr>` — the value is an ident or a path
        // (`crate::rules::x`); keep the last segment.
        Some(pos) => {
            let bytes = inner.as_bytes();
            let mut i = pos;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let start = i;
            while i < bytes.len()
                && (ident_continue(bytes[i])
                    || (bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':'))
            {
                i += 1;
            }
            let expr = inner[start..i].trim_end_matches(':');
            expr.rsplit("::").next().unwrap_or(expr).trim().to_owned()
        }
        // Field-init shorthand `check,` — the fn is `check`.
        None => "check".to_owned(),
    };
    if check.is_empty() {
        return None;
    }
    Some((name, check))
}

/// Every call-site in `body`: the callee's last-segment ident plus the
/// qualifier chain before it (`a::b::f(` → `["a","b"]`, `f`). A method
/// call (`x.f(`, `x?.f(`) returns `qual == None` — its callee lives on
/// the receiver's type, never in this module's fn namespace, so it must
/// not seed a free-fn edge (a bare `f(` and `x.f(` are otherwise
/// indistinguishable, and the global fallback would weld `check` to any
/// fold-reading `iter`/`kind`/`field` anywhere in the tree).
fn calls_in(text: &[u8], body: (usize, usize)) -> Vec<(Option<Vec<String>>, String)> {
    let mut calls = Vec::new();
    let (start, end) = body;
    let mut i = start;
    while i < end {
        if text[i] == b'(' {
            // Walk back over whitespace, then the ident/:: chain.
            let mut j = i;
            while j > start && text[j - 1].is_ascii_whitespace() {
                j -= 1;
            }
            let name_start = ident_before(text, j);
            if name_start == j {
                i += 1;
                continue;
            }
            let name = String::from_utf8_lossy(&text[name_start..j]).into_owned();
            // A `.`/`?.` directly before the callee makes it a method
            // call on the receiver — no module-path resolution applies.
            let mut p = name_start;
            while p > start && text[p - 1].is_ascii_whitespace() {
                p -= 1;
            }
            if p > start && text[p - 1] == b'.' {
                calls.push((None, name));
                i += 1;
                continue;
            }
            // Qualified chain: `::` + ident pairs preceding the name.
            let mut quals = Vec::new();
            let mut k = name_start;
            loop {
                let mut q = k;
                while q > start + 1 && text[q - 1].is_ascii_whitespace() {
                    q -= 1;
                }
                if q > start + 1 && text[q - 1] == b':' && text[q - 2] == b':' {
                    let qend = q - 2;
                    let qstart = ident_before(text, qend);
                    if qstart == qend {
                        break;
                    }
                    quals.push(String::from_utf8_lossy(&text[qstart..qend]).into_owned());
                    k = qstart;
                } else {
                    break;
                }
            }
            quals.reverse();
            calls.push((Some(quals), name));
        }
        i += 1;
    }
    calls
}

/// Build the call graph and mark every fn whose body reaches a
/// `.semantics()` read.
fn analyze(files: Vec<SourceFile>, texts: &[Vec<u8>]) -> Scan {
    let mut by_stem = HashMap::new();
    let mut by_name: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (fi, file) in files.iter().enumerate() {
        by_stem.insert(file.stem.clone(), fi);
        for (ni, f) in file.fns.iter().enumerate() {
            by_name.entry(f.name.clone()).or_default().push((fi, ni));
        }
    }
    // Inner-mod name → (file, module): a `mod_name::f(` qualifier.
    let mut by_mod: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (fi, file) in files.iter().enumerate() {
        for (mi, module) in file.modules.iter().enumerate() {
            let last = module.rsplit("::").next().unwrap_or(module);
            by_mod.entry(last.to_owned()).or_default().push((fi, mi));
        }
    }

    // `super::f` from a file-level item: `x/y.rs` in a subdir resolves
    // against the parent-dir module file (`expressions/foo.rs` →
    // `expressions.rs`, `declarations/foo.rs` → `declarations.rs`); a
    // `src/rules` top-level file resolves against `mod.rs`.
    let parent_of = |path: &str| -> Option<usize> {
        let dir = path.rsplit('/').nth(1)?;
        let parent_stem = if dir == "rules" { "mod" } else { dir };
        files.iter().position(|f| f.stem == parent_stem)
    };

    // edges[f] = callee fns its body may invoke.
    let mut edges: Vec<Vec<BTreeSet<(usize, usize)>>> = files
        .iter()
        .map(|f| vec![BTreeSet::new(); f.fns.len()])
        .collect();
    for (fi, file) in files.iter().enumerate() {
        for (ni, item) in file.fns.iter().enumerate() {
            for (quals, name) in calls_in(&texts[fi], item.body) {
                // A method call (`x.f(`) has no module-path callee — it
                // lives on the receiver's type and never seeds an edge.
                let Some(quals) = quals else {
                    continue;
                };
                let Some(candidates) = by_name.get(&name) else {
                    continue;
                };
                let mut resolved: Vec<(usize, usize)> = Vec::new();
                // A qualifier segment naming a module narrows the
                // candidates to fns inside it (`unused_futures::check`,
                // `const_ability::spells_meta_library`).
                // A qualifier segment is "internal" when it names a
                // crate-relative anchor (`crate`/`self`/`super`/`rules`),
                // an inner mod, or a rule file's stem — anything else is
                // an external crate (`dartq_scanner::scan`) whose callee
                // never lives in this tree. One foreign segment means the
                // call resolves outside the indexed fns entirely; welding
                // it to a same-named internal fn is the over-attribution
                // this branch exists to prevent.
                let mut all_internal = true;
                for qual in &quals {
                    if matches!(qual.as_str(), "crate" | "self" | "super" | "rules") {
                        continue;
                    }
                    if let Some(targets) = by_mod.get(qual) {
                        for &(tfi, tmi) in targets {
                            for &cand in candidates {
                                if cand.0 == tfi && files[cand.0].fns[cand.1].module == tmi {
                                    resolved.push(cand);
                                }
                            }
                        }
                    } else if let Some(&tfi) = by_stem.get(qual) {
                        for &cand in candidates {
                            if cand.0 == tfi {
                                resolved.push(cand);
                            }
                        }
                    } else {
                        all_internal = false;
                    }
                }
                if resolved.is_empty() {
                    if quals.iter().any(|q| q == "super") {
                        // `super::f` — the parent module's items (the
                        // file's own top level for an inner mod, the
                        // parent dir file otherwise).
                        if file.fns[ni].module != 0 {
                            for &cand in candidates {
                                if cand.0 == fi {
                                    resolved.push(cand);
                                }
                            }
                        } else if let Some(pfi) = parent_of(&file.path) {
                            for &cand in candidates {
                                if cand.0 == pfi {
                                    resolved.push(cand);
                                }
                            }
                        }
                    } else if quals.iter().any(|q| q == "self") {
                        for &cand in candidates {
                            if cand.0 == fi {
                                resolved.push(cand);
                            }
                        }
                    } else if quals.is_empty() {
                        // Unqualified: same module, then same file, then
                        // the crate-global name.
                        for &cand in candidates {
                            if cand.0 == fi
                                && files[cand.0].fns[cand.1].module == file.fns[ni].module
                            {
                                resolved.push(cand);
                            }
                        }
                        if resolved.is_empty() {
                            for &cand in candidates {
                                if cand.0 == fi {
                                    resolved.push(cand);
                                }
                            }
                        }
                        if resolved.is_empty() {
                            resolved.extend_from_slice(candidates);
                        }
                    } else if all_internal {
                        // `crate::rules::f` / `self::f` / `super::f` — the
                        // qualifier is fully crate-relative but named no
                        // module the stem/mod index knows (a re-exported
                        // `pub(crate) fn`, a `use`-imported helper). The
                        // crate-global name is the conservative edge.
                        resolved.extend_from_slice(candidates);
                    }
                    // else: a foreign qualifier (`dartq_scanner::`,
                    // `dartq_parser::`) — the callee lives outside the
                    // indexed rules tree and contributes no edge.
                }
                edges[fi][ni].extend(resolved);
            }
        }
    }

    // The fixed point: a fn reads the fold when its body holds a direct
    // `.semantics()` call or it calls a fn that does.
    let mut reads: Vec<Vec<bool>> = files
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            f.fns
                .iter()
                .map(|f| {
                    texts[fi][f.body.0..f.body.1]
                        .windows(b".semantics()".len())
                        .any(|w| w == b".semantics()")
                })
                .collect()
        })
        .collect();
    loop {
        let mut changed = false;
        for (fi, file) in files.iter().enumerate() {
            for ni in 0..file.fns.len() {
                if !reads[fi][ni] && edges[fi][ni].iter().any(|&(ef, en)| reads[ef][en]) {
                    reads[fi][ni] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    Scan {
        files,
        by_name,
        reads,
        edges,
    }
}

/// The direct `.semantics()` witness reachable from `(fi, ni)` through
/// `edges` — a BFS back-trace used only to name the propagation path in a
/// failure/debug report, never in the pass decision.
fn witness(scan: &Scan, texts: &[Vec<u8>], from: (usize, usize)) -> Vec<String> {
    let mut path = Vec::new();
    let mut seen = BTreeSet::new();
    let mut frontier = vec![from];
    while let Some((fi, ni)) = frontier.pop() {
        if !seen.insert((fi, ni)) {
            continue;
        }
        let f = &scan.files[fi].fns[ni];
        let is_direct = texts[fi][f.body.0..f.body.1]
            .windows(b".semantics()".len())
            .any(|w| w == b".semantics()");
        path.push(format!(
            "{}::{} {}",
            scan.files[fi].path,
            f.name,
            if is_direct { "(direct read)" } else { "" }
        ));
        if is_direct {
            break;
        }
        for &e in &scan.edges[fi][ni] {
            if scan.reads[e.0][e.1] {
                frontier.push(e);
            }
        }
    }
    path
}

/// The check fn a spec resolves to: same module first, then the file,
/// then the global name (`crate::rules::not_dispatched` lives in
/// `rules/mod.rs`).
fn resolve_check(scan: &Scan, fi: usize, spec: &SpecItem) -> Option<(usize, usize)> {
    let candidates = scan.by_name.get(&spec.check)?;
    // Same inner module (the aggregate files co-locate `fn check` with
    // their `static SPEC` inside `pub(crate) mod <rule>`).
    for &cand in candidates {
        if cand.0 == fi && scan.files[cand.0].fns[cand.1].module == spec.module {
            return Some(cand);
        }
    }
    for &cand in candidates {
        if cand.0 == fi {
            return Some(cand);
        }
    }
    candidates.first().copied()
}

/// The whole mount gate: the scan-derived consumer set is a subset of
/// `SEMANTIC_RULES` (modulo the allowlist), and the scanner saw every
/// registered rule's declaration — the two directions that make the
/// silence structurally impossible.
#[test]
fn every_fold_consumer_is_registered() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("src/rules");
    let mut paths = Vec::new();
    rule_files(&root, &mut paths);
    paths.sort();
    assert!(!paths.is_empty(), "no rule sources under {root:?}");

    let mut files = Vec::new();
    let mut texts = Vec::new();
    for path in &paths {
        let rel = path
            .strip_prefix(manifest)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let src = std::fs::read_to_string(path).expect("readable rule source");
        texts.push(strip_noise(&src));
        files.push(parse_file(path, &rel));
    }
    let scan = analyze(files, &texts);

    // Completeness of the scan itself: every registered rule's name was
    // seen at a declaration site — a spec form the extractor misses
    // would otherwise render the whole gate blind to that rule.
    let declared: BTreeSet<&str> = scan
        .files
        .iter()
        .flat_map(|f| f.specs.iter().map(|s| s.name.as_str()))
        .collect();
    let registered: BTreeSet<&str> = rules().iter().map(|s| s.name).collect();
    let unseen: Vec<&str> = registered.difference(&declared).copied().collect();
    let extra: Vec<&str> = declared.difference(&registered).copied().collect();
    assert!(
        unseen.is_empty() && extra.is_empty(),
        "the mount-gate scan lost track of rule declarations: \
         registered but never declared {unseen:?}; declared but never registered {extra:?}"
    );

    let table: BTreeSet<&str> = SEMANTIC_RULES.iter().copied().collect();
    let allow: BTreeMap<&str, &str> = ALLOWLIST.iter().copied().collect();
    let mut violations = Vec::new();
    for (fi, file) in scan.files.iter().enumerate() {
        for spec in &file.specs {
            let Some(check) = resolve_check(&scan, fi, spec) else {
                violations.push(format!(
                    "{}: rule {:?}'s check fn {:?} resolved to nothing \
                     (the scan cannot prove it stays off the fold)",
                    file.path, spec.name, spec.check
                ));
                continue;
            };
            if !scan.reads[check.0][check.1] {
                continue;
            }
            if table.contains(spec.name.as_str()) || allow.contains_key(spec.name.as_str()) {
                continue;
            }
            // Name the propagation path back to the direct `.semantics()`
            // read — the witness makes a real consumer obvious and a
            // scanner over-attribution debuggable in the same line.
            let path = witness(&scan, &texts, check).join(" -> ");
            violations.push(format!(
                "{}: rule {:?} reads the semantic fold ({}) \
                 but is absent from SEMANTIC_RULES — `mounts_semantics` \
                 never mounts the `Analysis` for it, so `c.semantics()` \
                 is always `None` and every row the rule would emit is \
                 silently dropped (the #3527/#3530 silent-death class). \
                 Register the name in src/registry.rs's SEMANTIC_RULES, \
                 or add an ALLOWLIST entry in tests/semantic_mount_gate.rs \
                 with the reason the unmounted read is correct.",
                file.path, spec.name, path
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "semantic-fold consumers missing from SEMANTIC_RULES:\n  {}",
        violations.join("\n  ")
    );
}

/// The three driver-mounted `Unit` faces: the byte token a rule's check
/// body reads (comments and literals are stripped before matching, so a
/// doc mention never registers), and whether the driver fills the face
/// from the semantic fold's `Analysis` facts. A fold-fed face's readers
/// must additionally sit in `SEMANTIC_RULES` — `mounts_semantics` is what
/// builds the `Analysis` the driver reads the rows from, so a gate name
/// left out of that table passes its own gate yet mounts an empty face
/// (the same silent death behind one extra table away).
const DRIVER_FACES: &[(&str, &[u8], bool)] = &[
    (
        "Unit::async_context_uses",
        b".unit.async_context_uses",
        true,
    ),
    ("Unit::reachability", b".unit.reachability", false),
    ("Unit::library_type_uses", b".library_type_uses", false),
];

/// The lint driver's production gate names: every `rule_by_name("...")`
/// literal outside a test module, in file order. The same noise-strip and
/// test-mod-skip discipline as [`parse_file`] — a doc mention or a
/// test's registry lookup must never register as a production seam gate.
fn driver_gate_names(path: &Path) -> Vec<String> {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let text = strip_noise(&src);

    // The test-module byte ranges: a `mod tests`/`mod test` body (or a
    // `#[cfg(test)]`-gated one) is fixture code, not production seams.
    let mut test_mods: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if keyword_at(&text, i, "mod") {
            let mut j = i + 3;
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            let name_end = ident_after(&text, j);
            if name_end > j {
                let name = &src[j..name_end];
                let mut k = name_end;
                while k < text.len() && text[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < text.len() && text[k] == b'{' {
                    let end = balanced(&text, k, b'{', b'}');
                    let cfg_test = text[i.saturating_sub(64)..i]
                        .windows(b"cfg(test)".len())
                        .any(|w| w == b"cfg(test)");
                    let is_test = cfg_test || name == "tests" || name == "test";
                    if is_test {
                        test_mods.push((i, end));
                        i = end;
                    } else {
                        // A production inner mod's body keeps scanning —
                        // its items may gate seams too.
                        i = k + 1;
                    }
                    continue;
                }
            }
            i += 3;
            continue;
        }
        i += 1;
    }

    // The gate sites: `rule_by_name(` — the name is the first string
    // literal of the arg list, read from the raw source.
    let mut names = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if keyword_at(&text, i, "rule_by_name") {
            let mut j = i + "rule_by_name".len();
            while j < text.len() && text[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < text.len() && text[j] == b'(' {
                let end = balanced(&text, j, b'(', b')');
                let stop = end.saturating_sub(1).max(j + 1);
                let in_test = test_mods.iter().any(|&(s, e)| i >= s && i < e);
                if !in_test && let Some(name) = first_string_literal(&src[j + 1..stop]) {
                    names.push(name);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    names
}

/// The first string literal's content in a raw arg slice (a rule name
/// carries no escapes).
fn first_string_literal(inner: &str) -> Option<String> {
    let start = inner.find('"')?;
    let stop = inner[start + 1..].find('"')? + start + 1;
    Some(inner[start + 1..stop].to_owned())
}

/// Every fn whose call graph reaches a direct read of `token` — the same
/// fixed point [`analyze`] runs for `.semantics()`, seeded with a
/// different mount face.
fn reads_face(scan: &Scan, texts: &[Vec<u8>], token: &[u8]) -> Vec<Vec<bool>> {
    let mut reads: Vec<Vec<bool>> = scan
        .files
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            f.fns
                .iter()
                .map(|f| {
                    texts[fi][f.body.0..f.body.1]
                        .windows(token.len())
                        .any(|w| w == token)
                })
                .collect()
        })
        .collect();
    loop {
        let mut changed = false;
        for (fi, file) in scan.files.iter().enumerate() {
            for ni in 0..file.fns.len() {
                if !reads[fi][ni] && scan.edges[fi][ni].iter().any(|&(ef, en)| reads[ef][en]) {
                    reads[fi][ni] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    reads
}

/// The driver-seam gate: the lint driver mounts the three `Unit` faces
/// under per-rule-name gates (`rule_by_name("...")` in
/// `crates/dartq-commands/src/lints.rs`), and the rule side reads the
/// faces off the mounted unit — so the two ends agree or the rows die
/// silently. Two directions, both derived from source, no per-rule
/// boilerplate:
///
/// - every name the driver gates must be a live registered rule. A
///   drifted literal resolves `rule_by_name` to `None`, the gate stays
///   permanently false, the face never mounts, and every row the gated
///   rule would emit is silently dropped; a gate on a removed rule is
///   the same dead seam with extra mount work.
/// - every rule the scan proves reads a mounted face must be one of the
///   names the driver gates (and, for a fold-fed face, a `SEMANTIC_RULES`
///   member — the fold is what fills the face's rows). A new reader whose
///   gate was never added fails here, naming the file and rule.
#[test]
fn every_driver_seam_gate_matches_the_registry_and_the_face_readers() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let driver = manifest
        .parent()
        .expect("crates dir")
        .join("dartq-commands/src/lints.rs");
    let gates: BTreeSet<String> = driver_gate_names(&driver).into_iter().collect();
    assert!(
        !gates.is_empty(),
        "no rule_by_name gates found in {driver:?} — the scan lost the driver"
    );

    let mut violations = Vec::new();
    for name in &gates {
        match rules().iter().find(|spec| spec.name == name.as_str()) {
            None => violations.push(format!(
                "the lint driver ({driver:?}) gates a seam on {name:?} but no \
                 such rule is registered — `rule_by_name` resolves to `None`, \
                 the gate stays permanently false and the face is never \
                 mounted: every row the gated rule would emit is silently \
                 dropped. Fix the name in dartq-commands/src/lints.rs or \
                 register the rule."
            )),
            // `Group::Removed` is the registry's own removal marker (the
            // `removed_rule!` family: `check: not_dispatched`). `kinds`
            // emptiness is NOT a removal signal — the seam-driven rules
            // (pubspec pass, run epilogue, breaking-version gate) carry
            // `kinds: &[]` by design: the driver runs them through their
            // seam, not through AST-node dispatch.
            Some(spec) if spec.group == Group::Removed => {
                violations.push(format!(
                    "the lint driver ({driver:?}) gates a seam on {name:?} but \
                     the rule is removed (`Group::Removed`, the registered \
                     no-op family) — the gate mounts a face nothing will ever \
                     read. Retire the seam with the rule in \
                     dartq-commands/src/lints.rs."
                ));
            }
            Some(_) => {}
        }
    }

    // The reader side: the same rules-tree scan the fold gate runs, with
    // each driver-mounted face's token as the read being traced.
    let root = manifest.join("src/rules");
    let mut paths = Vec::new();
    rule_files(&root, &mut paths);
    paths.sort();
    assert!(!paths.is_empty(), "no rule sources under {root:?}");
    let mut files = Vec::new();
    let mut texts = Vec::new();
    for path in &paths {
        let rel = path
            .strip_prefix(manifest)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let src = std::fs::read_to_string(path).expect("readable rule source");
        texts.push(strip_noise(&src));
        files.push(parse_file(path, &rel));
    }
    let scan = analyze(files, &texts);

    for &(face, token, fold_fed) in DRIVER_FACES {
        let reads = reads_face(&scan, &texts, token);
        let mut readers: Vec<&str> = Vec::new();
        for (fi, file) in scan.files.iter().enumerate() {
            for spec in &file.specs {
                if let Some(check) = resolve_check(&scan, fi, spec)
                    && reads[check.0][check.1]
                {
                    readers.push(spec.name.as_str());
                }
            }
        }
        readers.sort_unstable();
        readers.dedup();
        for reader in readers {
            if !gates.contains(reader) {
                violations.push(format!(
                    "{reader:?} reads the driver-mounted {face} face but the \
                     lint driver ({driver:?}) never gates it by name — the \
                     face stays unmounted (`None`), the check bails on its \
                     `let Some(..) = .. else {{ return }}`, and every row the \
                     rule would emit is silently dropped (the #3530 silent-\
                     death class at the driver seam). Add the rule's name to \
                     the seam's `rule_by_name` gate in \
                     dartq-commands/src/lints.rs."
                ));
            }
            if fold_fed && !SEMANTIC_RULES.iter().any(|&name| name == reader) {
                violations.push(format!(
                    "{reader:?} reads the fold-fed {face} face but is absent \
                     from SEMANTIC_RULES — the driver fills that face from \
                     the `Analysis` facts, and `mounts_semantics` (the fold \
                     mount) never fires for a run enabling only this rule, \
                     so the rows are empty and the rule is silently dead \
                     even though the driver's own gate agrees. Register the \
                     name in src/registry.rs's SEMANTIC_RULES."
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "driver-seam mount drift between dartq-commands/src/lints.rs and \
         the rule registry:\n  {}",
        violations.join("\n  ")
    );
}
