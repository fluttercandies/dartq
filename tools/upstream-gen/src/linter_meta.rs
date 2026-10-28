//! The linter metadata reader: derives the checked-in lint rule metadata
//! table from the pinned upstream sources (#2676).
//!
//! Sources, all under the SDK tree unless noted:
//!
//! - `pkg/linter/messages.yaml` — the upstream source of truth. Keyed by
//!   camelCase const under `LinterLintCode:`; the per-const fields used
//!   here: `sharedName`, `problemMessage`, `correctionMessage`,
//!   `hasPublishedDocs`, `parameters` (declaration-ordered parameter
//!   names), `state` (the removed/deprecated/stable/... sub-keys). The
//!   yaml's own header instructs upstream's generation
//!   (`tool/generate_lints.dart`).
//! - `pkg/linter/lib/src/lint_names.g.dart` — the generated machine-name
//!   set; every derived name must match it exactly (the cross-check that
//!   pins the camel→snake conversion).
//! - `pkg/linter/lib/src/rules/*.dart` — per-rule registrations: the
//!   canonical diagnostic code(s) (`DiagnosticCode get diagnosticCode` /
//!   `diagnosticCodes => [...]`) and the removed/deprecated `since`
//!   versions the option faces render (`RemovedAnalysisRule(since: ...)`,
//!   `state: RuleState.deprecated(since: ...)`; the `.deprecated(...)` and
//!   `.new(a, b, c)` dot-shorthands included).
//! - `pkg/analyzer/lib/analysis_rule/rule_state.dart` — the `dartX_Y`
//!   version constants the since registrations reference.
//! - the pinned `package:lints` snapshot's `lib/core.yaml` /
//!   `lib/recommended.yaml` (a separate local tree) — group membership.
//!
//! The machine name of a const is `snake_case(sharedName or const)` with
//! `_` inserted at letter→upper and letter→digit boundaries
//! (`linesLongerThan80Chars` → `lines_longer_than_80_chars`), exactly the
//! generated `LintNames` spellings. Message `#param` references resolve to
//! `{i}` positional slots through the const's `parameters` declaration
//! order (`#p0` included — the analyzer's positional spelling names the
//! same slots), the fill the dartq templates render.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One `LinterLintCode` const of `pkg/linter/messages.yaml`.
#[derive(Debug, Clone)]
pub struct ConstEntry {
    pub const_name: String,
    /// snake_case machine name (sharedName or the const itself).
    pub name: String,
    pub problem: Option<String>,
    pub correction: Option<String>,
    pub published: bool,
    pub removed: bool,
    pub deprecated: bool,
    /// Parameter names in declaration order (last word of each key).
    pub params: Vec<String>,
}

/// One folded rule row — the generated table's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRow {
    pub name: String,
    /// `core` | `recommended` | `standalone` | `removed`.
    pub group: &'static str,
    pub problem: String,
    pub correction: String,
    /// `stable` | `removed` | `deprecated` (the live-but-unconsumed
    /// states — experimental/internal/testing — all fold to `stable`).
    pub state: &'static str,
    /// The since version the option face renders, `None` unversioned.
    pub since: Option<String>,
    pub published: bool,
}

/// Canonical-code selection overrides: multi-variant families where the
/// dartq message of record is a different const than the mechanical chain
/// picks. Each is an upstream const — the texts stay byte-identical; only
/// the family's representative is pinned here.
const SELECTION_OVERRIDES: &[(&str, &str)] = &[
    (
        "avoid_types_as_parameter_names",
        "avoidTypesAsParameterNamesFormalParameter",
    ),
    (
        "deprecated_member_use_from_same_package",
        "deprecatedMemberUseFromSamePackageWithoutMessage",
    ),
    (
        "invalid_runtime_check_with_js_interop_types",
        "invalidRuntimeCheckWithJsInteropTypesDartIsJs",
    ),
    ("prefer_contains", "preferContainsUseContains"),
    ("prefer_final_in_for_each", "preferFinalInForEachVariable"),
    ("prefer_is_empty", "preferIsEmptyUseIsEmpty"),
    (
        "prefer_typing_uninitialized_variables",
        "preferTypingUninitializedVariablesForLocalVariable",
    ),
    ("unnecessary_final", "unnecessaryFinalWithoutType"),
    ("use_super_parameters", "useSuperParametersSingle"),
];

/// Pinned wordings: rules whose dartq message of record matches no const
/// of the pinned tree. `deprecated_consistency`'s official codes are all
/// site-specific (constructor/field/parameter); the dartq rule reports
/// each site with its official text and the spec's record stays the
/// generic face, which no site renders.
const PINNED_WORDINGS: &[(&str, &str, &str)] = &[(
    "deprecated_consistency",
    "Missing deprecated annotation.",
    "",
)];

/// `snake_case` machine-name derivation (the `LintNames` generator's
/// spelling): `_` before an uppercase letter following a letter/digit, and
/// before a digit following a letter.
#[must_use]
pub fn snake_case(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 8);
    for (i, ch) in camel.chars().enumerate() {
        if i > 0 && ch.is_ascii_uppercase() {
            out.push('_');
        } else if i > 0
            && ch.is_ascii_digit()
            && out.chars().last().is_some_and(|p| p.is_ascii_alphabetic())
        {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// Parses `pkg/linter/messages.yaml` into const entries, in file order
/// (serde_yaml mappings preserve document order — the fold's tie-break).
pub fn parse_messages(src: &str) -> Result<Vec<ConstEntry>, String> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(src).map_err(|e| format!("pkg/linter/messages.yaml: {e}"))?;
    let root = yaml
        .as_mapping()
        .ok_or("pkg/linter/messages.yaml: root is not a mapping")?;
    let entries = root
        .get(serde_yaml::Value::String("LinterLintCode".to_owned()))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or("pkg/linter/messages.yaml: no LinterLintCode mapping")?;
    let mut out = Vec::with_capacity(entries.len());
    for (const_name, value) in entries {
        let Some(const_name) = const_name.as_str() else {
            continue;
        };
        let Some(fields) = value.as_mapping() else {
            return Err(format!("{const_name}: entry is not a mapping"));
        };
        let get_str = |key: &str| -> Option<String> {
            fields
                .get(serde_yaml::Value::String(key.to_owned()))
                .and_then(serde_yaml::Value::as_str)
                .map(str::to_owned)
        };
        let mut params = Vec::new();
        if let Some(serde_yaml::Value::Mapping(map)) =
            fields.get(&serde_yaml::Value::String("parameters".to_owned()))
        {
            for key in map.keys() {
                let Some(key) = key.as_str() else { continue };
                params.push(key.rsplit(' ').next().unwrap_or(key).to_owned());
            }
        }
        let mut removed = false;
        let mut deprecated = false;
        if let Some(serde_yaml::Value::Mapping(state)) =
            fields.get(&serde_yaml::Value::String("state".to_owned()))
        {
            for key in state.keys() {
                match key.as_str() {
                    Some("removed") => removed = true,
                    Some("deprecated") => deprecated = true,
                    _ => {}
                }
            }
        }
        let machine = get_str("sharedName").unwrap_or_else(|| const_name.to_owned());
        out.push(ConstEntry {
            const_name: const_name.to_owned(),
            name: snake_case(&machine),
            problem: get_str("problemMessage"),
            correction: get_str("correctionMessage"),
            published: fields
                .get(&serde_yaml::Value::String("hasPublishedDocs".to_owned()))
                .and_then(serde_yaml::Value::as_bool)
                .unwrap_or(false),
            removed,
            deprecated,
            params,
        });
    }
    if out.is_empty() {
        return Err("pkg/linter/messages.yaml: no LinterLintCode entries".to_owned());
    }
    Ok(out)
}

/// The name-set cross-check: derived names must equal the generated
/// `lint_names.g.dart` constants exactly — a new upstream naming shape
/// fails the run instead of silently mis-deriving names.
pub fn validate_names(entries: &[ConstEntry], lint_names_src: &str) -> Result<(), String> {
    let mut official: Vec<&str> = Vec::new();
    for line in lint_names_src.lines() {
        let Some(rest) = line.trim().strip_prefix("static const String ") else {
            continue;
        };
        let Some(name) = rest.split(' ').next() else { continue };
        if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            official.push(name);
        }
    }
    let derived: Vec<&str> = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut sorted: Vec<&str> = official.clone();
    sorted.sort_unstable();
    official = sorted;
    if derived != official {
        let extra: Vec<&str> = derived
            .iter()
            .filter(|name| !official.contains(name))
            .copied()
            .collect();
        let missing: Vec<&str> = official
            .iter()
            .filter(|name| !derived.contains(name))
            .copied()
            .collect();
        return Err(format!(
            "derived machine names disagree with lint_names.g.dart \
             (derived-only {extra:?}, generated-only {missing:?})"
        ));
    }
    Ok(())
}

/// The `dartX_Y` version constants of `rule_state.dart`, as
/// `major.minor.patch` texts.
pub fn version_consts(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("final Version ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(" = ") else {
            continue;
        };
        let Some(version) = parse_version_ctor(tail.trim_end_matches(';')) else {
            continue;
        };
        out.insert(name.trim().to_owned(), version);
    }
    out
}

/// `Version(a, b, c)` / `.new(a, b, c)` → `"a.b.c"`.
fn parse_version_ctor(text: &str) -> Option<String> {
    let inner = text
        .strip_prefix("Version(")
        .or_else(|| text.strip_prefix(".new("))
        .map(|rest| rest.trim_end_matches(')'))?;
    let mut parts = inner.split(',');
    let mut nums = [0u32; 3];
    for slot in &mut nums {
        let part = parts.next()?.trim();
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(format!("{}.{}.{}", nums[0], nums[1], nums[2]))
}

/// A `since:` argument inside a registration block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SinceArg {
    Version(String),
    /// A `dartX_Y` constant reference, resolved against
    /// [`version_consts`] by the fold.
    Const(String),
}

/// Per-rule registration facts extracted from one rule file (opaque to
/// the bin — read through [`scan_rule_files`] and [`fold`]).
#[derive(Debug, Default)]
pub struct RuleFileFacts {
    /// Canonical code consts in declaration order (the `diagnosticCode`
    /// getter first, else the `diagnosticCodes` list).
    pub(crate) codes: Vec<String>,
    /// The removed registration and its `since`, when the rule has one.
    pub(crate) removed_since: Option<Option<SinceArg>>,
    pub(crate) deprecated_since: Option<Option<SinceArg>>,
    pub(crate) replaced_by: bool,
}

/// Scans `pkg/linter/lib/src/rules/*.dart` (the name and text of every
/// file) into per-rule facts, keyed by machine name.
pub fn scan_rule_files(files: &[(String, String)]) -> BTreeMap<String, RuleFileFacts> {
    let mut facts: BTreeMap<String, RuleFileFacts> = BTreeMap::new();
    for (_file, src) in files {
        let anchors: Vec<usize> = src
            .match_indices("name: LintNames.")
            .map(|(at, _)| at)
            .collect();
        for (i, &at) in anchors.iter().enumerate() {
            let Some(name) = lint_name_at(src, at) else {
                continue;
            };
            let end = anchors.get(i + 1).copied().unwrap_or(src.len());
            if let Some(codes) = canonical_codes(&src[at..end]) {
                facts.entry(name).or_default().codes = codes;
            }
        }
        // The removed registration: `RemovedAnalysisRule(name:
        // LintNames.x, ..., since: <version>, ...)` up to the closing
        // `);` — the analyzer's `RemovedAnalysisRule` files close the
        // constructor before any nested call does.
        if let Some(at) = src.find("RemovedAnalysisRule(") {
            let end = src[at..].find(");").map_or(src.len(), |e| at + e);
            let block = &src[at..end];
            if let Some(name_at) = block.find("name: LintNames.")
                && let Some(name) = lint_name_at(block, name_at)
            {
                facts.entry(name).or_default().removed_since = Some(since_in_block(block));
            }
        }
        // The deprecated state: `state: RuleState.deprecated(since: V)` or
        // the `.deprecated(since: V)` dot-shorthand, attributed to the
        // nearest preceding `name: LintNames.<x>` registration. The block
        // runs to the call's balanced closing paren so nested version
        // ctors stay whole.
        if let Some(state_at) = src.find("deprecated(")
            && let Some(name_at) = src[..state_at].rfind("name: LintNames.")
            && let Some(name) = lint_name_at(src, name_at)
        {
            let open = state_at + "deprecated".len();
            let mut depth = 0usize;
            let mut end = src.len();
            for (i, b) in src.as_bytes().iter().enumerate().skip(open) {
                match b {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let block = &src[state_at..end];
            let entry = facts.entry(name).or_default();
            entry.deprecated_since = Some(since_in_block(block));
            entry.replaced_by = block.contains("replacedBy:");
        }
    }
    facts
}

/// The `LintNames.<x>` identifier at `at`, snake_cased.
fn lint_name_at(src: &str, at: usize) -> Option<String> {
    let rest = &src[at + "name: LintNames.".len()..];
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!ident.is_empty()).then(|| snake_case(&ident))
}

/// The canonical codes a class region declares: the single
/// `diagnosticCode => diag.x` getter or the `diagnosticCodes => [...]`
/// list, in order.
fn canonical_codes(region: &str) -> Option<Vec<String>> {
    const GETTER: &str = "diagnosticCode => diag.";
    if let Some(at) = region.find(GETTER) {
        return Some(vec![diag_ident(&region[at + GETTER.len()..])]);
    }
    let start = region.find("diagnosticCodes => [")?;
    let open = start + "diagnosticCodes => ".len();
    let mut depth = 0usize;
    let mut end = region.len();
    for (i, b) in region.as_bytes().iter().enumerate().skip(open) {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    Some(
        region[open..end]
            .split("diag.")
            .skip(1)
            .map(diag_ident)
            .collect(),
    )
}

/// The identifier at the start of `text` (a `diag.` reference's target).
fn diag_ident(text: &str) -> String {
    text.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// The `since:` argument inside a registration block: a version ctor or a
/// `dartX_Y` constant; `None` when the registration carries no since (the
/// unversioned face). The value ends at a comma or paren at depth zero,
/// so nested ctors (`.new(3, 13, 0)`) parse whole.
fn since_in_block(block: &str) -> Option<SinceArg> {
    let at = block.find("since:")?;
    let rest = &block[at + "since:".len()..];
    let mut value = String::new();
    let mut depth = 0usize;
    for ch in rest.chars() {
        if ch == '(' {
            depth += 1;
            value.push(ch);
        } else if ch == ')' {
            if depth > 0 {
                depth -= 1;
                value.push(ch);
            } else {
                break;
            }
        } else if ch == ',' && depth == 0 {
            break;
        } else {
            value.push(ch);
        }
    }
    let value = value.trim();
    if let Some(version) = parse_version_ctor(value) {
        return Some(SinceArg::Version(version));
    }
    if value.starts_with("dart") && !value.contains('(') {
        return Some(SinceArg::Const(value.to_owned()));
    }
    None
}

/// The `linter.rules` names of a `package:lints` set yaml, lowercased to
/// machine names (the snapshot spells entries as plain names).
pub fn lints_set_names(src: &str) -> Result<Vec<String>, String> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(src).map_err(|e| format!("lints set: {e}"))?;
    let rules = yaml
        .get("linter")
        .and_then(|linter| linter.get("rules"))
        .ok_or("lints set: no linter.rules list")?;
    let Some(items) = rules.as_sequence() else {
        return Err("lints set: linter.rules is not a list".to_owned());
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(|name| name.to_ascii_lowercase())
                .ok_or_else(|| "lints set: non-name rules entry".to_owned())
        })
        .collect()
}

/// The fold: const entries → name-sorted rule rows.
///
/// Canonical-code selection per name (the multi-variant families): the
/// [`SELECTION_OVERRIDES`] pin, else the const named exactly like the rule
/// (`lowerCamel(name)`), else the rule file's first declared code, else
/// the first const in yaml order. Removed rules carry no problemMessage
/// upstream (the option face renders its own `removed_lint` texts), so
/// their row messages stay empty.
///
/// Every check is total — an inconsistency names the rule and aborts
/// rather than emitting a partial table.
pub fn fold(
    entries: &[ConstEntry],
    facts: &BTreeMap<String, RuleFileFacts>,
    versions: &BTreeMap<String, String>,
    core: &[String],
    recommended: &[String],
) -> Result<Vec<RuleRow>, String> {
    // The family map preserves first-const order (the last tie-break).
    let mut order: Vec<String> = Vec::new();
    let mut families: BTreeMap<String, Vec<&ConstEntry>> = BTreeMap::new();
    for entry in entries {
        if !families.contains_key(&entry.name) {
            order.push(entry.name.clone());
        }
        families.entry(entry.name.clone()).or_default().push(entry);
    }
    let mut rows = Vec::with_capacity(families.len());
    for name in &order {
        let family = &families[name];
        let removed = family.iter().any(|entry| entry.removed);
        let deprecated = family.iter().any(|entry| entry.deprecated);
        if removed && deprecated {
            return Err(format!("{name}: removed and deprecated at once"));
        }
        // The docs flag folds any-variant: upstream itself mixes flags
        // within a shared name (always_specify_types — one variant
        // published, three not), and the URL face the table feeds is
        // name-keyed, so any-published is the fold it serves.
        let published = family.iter().any(|entry| entry.published);
        let pick = |const_name: &str| {
            family
                .iter()
                .find(|entry| entry.const_name == const_name)
                .copied()
        };
        let selected = if let Some((_, const_name)) =
            SELECTION_OVERRIDES.iter().find(|(rule, _)| rule == name)
        {
            pick(const_name)
                .ok_or_else(|| format!("{name}: selection override names unknown const {const_name}"))?
        } else {
            let base: String = {
                let mut chars = name.chars();
                let first = chars.next().expect("non-empty name");
                let mut out = first.to_ascii_uppercase().to_string();
                out.push_str(&name[first.len_utf8()..]);
                out
            };
            family
                .iter()
                .find(|entry| entry.const_name == base)
                .copied()
                .map_or_else(
                    || {
                        facts
                            .get(name)
                            .and_then(|file| file.codes.first())
                            .and_then(|const_name| pick(const_name))
                            .unwrap_or(family[0])
                    },
                    |entry| entry,
                )
        };
        let (problem, correction) = if removed {
            (String::new(), String::new())
        } else if let Some((_, problem, correction)) =
            PINNED_WORDINGS.iter().find(|(rule, _, _)| rule == name)
        {
            ((*problem).to_owned(), (*correction).to_owned())
        } else {
            (
                convert_refs(
                    selected.problem.as_deref().unwrap_or_default(),
                    &selected.params,
                    name,
                )?,
                convert_refs(
                    selected.correction.as_deref().unwrap_or_default(),
                    &selected.params,
                    name,
                )?,
            )
        };
        let group = if removed {
            if core.contains(name) || recommended.contains(name) {
                return Err(format!("{name}: a removed rule in a package:lints set"));
            }
            "removed"
        } else if core.contains(name) || recommended.contains(name) {
            if deprecated {
                return Err(format!(
                    "{name}: a deprecated lints-set member — rule on it before regenerating"
                ));
            }
            if core.contains(name) {
                "core"
            } else {
                "recommended"
            }
        } else {
            "standalone"
        };
        let since = if removed || deprecated {
            let field = if removed {
                facts
                    .get(name)
                    .and_then(|file| file.removed_since.as_ref())
                    .ok_or_else(|| format!("{name}: no rule-file registration for its state"))?
            } else {
                facts
                    .get(name)
                    .and_then(|file| file.deprecated_since.as_ref())
                    .ok_or_else(|| format!("{name}: no rule-file registration for its state"))?
            };
            if deprecated && facts.get(name).is_some_and(|file| file.replaced_by) {
                return Err(format!(
                    "{name}: a deprecated registration with replacedBy — the dartq \
                     option face renders the plain row only; rule on this name \
                     before regenerating"
                ));
            }
            resolve_since(field.as_ref(), versions, name)?
        } else {
            None
        };
        rows.push(RuleRow {
            name: name.clone(),
            group,
            problem,
            correction,
            state: if removed {
                "removed"
            } else if deprecated {
                "deprecated"
            } else {
                "stable"
            },
            since,
            published,
        });
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// Resolves a [`SinceArg`] against the version constants.
fn resolve_since(
    arg: Option<&SinceArg>,
    versions: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<String>, String> {
    match arg {
        None => Ok(None),
        Some(SinceArg::Version(text)) => Ok(Some(text.clone())),
        Some(SinceArg::Const(ident)) => versions.get(ident).cloned().map(Some).ok_or_else(|| {
            format!("{name}: since constant {ident} missing from rule_state.dart")
        }),
    }
}

/// Converts `#param` references to `{i}` positional slots through the
/// declaration-ordered parameter names; an unresolved reference fails the
/// run (a message dartq could never render).
fn convert_refs(text: &str, params: &[String], name: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('#') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let ident: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let Some(slot) = params.iter().position(|param| *param == ident) else {
            return Err(format!("{name}: message references #{ident} outside its parameters"));
        };
        let _ = write!(out, "{{{slot}}}");
        rest = &tail[ident.len()..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `core` → `Core` (the generated `Group` variant spellings).
fn group_camel(group: &str) -> &'static str {
    match group {
        "core" => "Core",
        "recommended" => "Recommended",
        "standalone" => "Standalone",
        "removed" => "Removed",
        _ => unreachable!("fold emits exactly these groups"),
    }
}

/// Renders `crates/dartq-lints/src/generated/rule_meta.rs` — a pure
/// function of the rows, the fold counts, and the SDK version stamp, so
/// regeneration is byte-reproducible for identical inputs.
#[must_use]
pub fn render_rule_meta(rows: &[RuleRow], const_count: usize, sdk_version: &str) -> String {
    const REGEN: &str = "cargo run -p upstream-gen --bin linter-meta-gen -- --sdk-src <dart-sdk-tree> --lints-src <lints-tree>";
    let mut out = String::with_capacity(4096 + rows.len() * 192);
    out.push_str("//! GENERATED lint rule metadata — DO NOT EDIT.\n");
    out.push_str("//! Regenerate with:\n");
    let _ = writeln!(out, "//!   {REGEN}");
    let _ = writeln!(
        out,
        "//! Source: pkg/linter/messages.yaml of the pinned SDK checkout ({sdk_version}),"
    );
    out.push_str("//!   canonical code selection from the pkg/linter/lib/src/rules/*.dart\n");
    out.push_str("//!   registrations, group sets from the pinned package:lints snapshot\n");
    out.push_str("//!   (lib/core.yaml, lib/recommended.yaml), removed/deprecated since\n");
    out.push_str("//!   versions from those registrations (version constants of\n");
    out.push_str("//!   pkg/analyzer/lib/analysis_rule/rule_state.dart).\n");
    let _ = writeln!(
        out,
        "//! {} consts fold to {} name rows; the selection chain, its overrides,",
        const_count,
        rows.len()
    );
    out.push_str("//!   and the pinned wordings are documented in tools/upstream-gen/src/\n");
    out.push_str("//!   linter_meta.rs. Rows are sorted by name; messages render '{i}'\n");
    out.push_str("//!   positional slots (the yaml '#param' references resolved through\n");
    out.push_str("//!   the const's parameter order). Removed rules carry no message —\n");
    out.push_str("//!   the options face renders the removed_lint templates.\n\n");
    out.push_str("use crate::registry::Group;\n\n");
    out.push_str(
        "/// The upstream rule state the option faces render. `Stable` covers\n\
         /// every live state (stable/experimental/internal/testing — none of\n\
         /// those carries a face).\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub enum MetaState {\n\
         \x20   Stable,\n\
         \x20   /// `RemovedAnalysisRule`, with the registration's `since` version\n\
         \x20   /// (`None` — the unversioned face — for `avoid_unstable_final_fields`).\n\
         \x20   Removed(Option<&'static str>),\n\
         \x20   /// `RuleState.deprecated`, with the registration's `since` version.\n\
         \x20   Deprecated(Option<&'static str>),\n\
         }\n\n",
    );
    out.push_str(
        "/// One rule's upstream-derived metadata — the single generated source\n\
         /// for the group, message templates, rule state, and docs flag the\n\
         /// registry, the option faces, and the documentation URL face read.\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub struct RuleMeta {\n\
         \x20   pub name: &'static str,\n\
         \x20   pub group: Group,\n\
         \x20   pub problem: &'static str,\n\
         \x20   pub correction: &'static str,\n\
         \x20   pub state: MetaState,\n\
         \x20   pub docs_published: bool,\n\
         }\n\n",
    );
    out.push_str("const TABLE: &[RuleMeta] = &[\n");
    for row in rows {
        let state = match (row.state, row.since.as_deref()) {
            ("stable", _) => "MetaState::Stable".to_owned(),
            ("removed", Some(since)) => format!("MetaState::Removed(Some({since:?}))"),
            ("removed", None) => "MetaState::Removed(None)".to_owned(),
            ("deprecated", Some(since)) => format!("MetaState::Deprecated(Some({since:?}))"),
            ("deprecated", None) => "MetaState::Deprecated(None)".to_owned(),
            _ => unreachable!("fold emits exactly these states"),
        };
        let _ = writeln!(
            out,
            "    RuleMeta {{ name: {:?}, group: Group::{}, problem: {:?}, correction: {:?}, state: {}, docs_published: {} }},",
            row.name,
            group_camel(row.group),
            row.problem,
            row.correction,
            state,
            row.published
        );
    }
    out.push_str("];\n\n");
    out.push_str(&format!(
        r#"/// The name-sorted metadata table — the runtime read face.
pub static RULE_META: &[RuleMeta] = TABLE;

/// The metadata of rule `name`, `None` when the name is not a first-party
/// linter rule of the pinned tree (the diagnostic mirrors and unknown
/// names). Binary search; a once-per-run lookup face.
#[must_use]
pub fn rule_meta(name: &str) -> Option<&'static RuleMeta> {{
    RULE_META
        .binary_search_by(|meta| meta.name.cmp(name))
        .ok()
        .map(|index| &RULE_META[index])
}}

/// The compile-time face: the `spec!` macro resolves a rule's group at
/// its `static SPEC` initializer, so a rule the table does not carry
/// fails the build (not the first query). One linear scan per site.
pub(crate) const fn meta_group(name: &str) -> Group {{
    let mut i = 0;
    while i < TABLE.len() {{
        if str_eq(TABLE[i].name, name) {{
            return TABLE[i].group;
        }}
        i += 1;
    }}
    panic!("lint rule has no generated metadata row; regenerate the table or add the name to the registry exemption list");
}}

/// The compile-time `problemMessage` face of [`meta_group`].
pub(crate) const fn meta_problem(name: &str) -> &'static str {{
    let mut i = 0;
    while i < TABLE.len() {{
        if str_eq(TABLE[i].name, name) {{
            return TABLE[i].problem;
        }}
        i += 1;
    }}
    panic!("lint rule has no generated metadata row; regenerate the table or add the name to the registry exemption list");
}}

/// The compile-time `correctionMessage` face of [`meta_group`].
pub(crate) const fn meta_correction(name: &str) -> &'static str {{
    let mut i = 0;
    while i < TABLE.len() {{
        if str_eq(TABLE[i].name, name) {{
            return TABLE[i].correction;
        }}
        i += 1;
    }}
    panic!("lint rule has no generated metadata row; regenerate the table or add the name to the registry exemption list");
}}

const fn str_eq(a: &str, b: &str) -> bool {{
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {{
        return false;
    }}
    let mut i = 0;
    while i < a.len() {{
        if a[i] != b[i] {{
            return false;
        }}
        i += 1;
    }}
    true
}}
"#
    ));
    out
}

/// Renders `crates/dartq-diagnostics/src/generated/lint_docs_table.rs` —
/// the `("name", published)` row slice `lint_docs.rs` includes into its
/// table (a single expression, the shape `include!` requires).
#[must_use]
pub fn render_lint_docs_rows(rows: &[RuleRow]) -> String {
    let mut out = String::with_capacity(rows.len() * 40 + 256);
    out.push_str("// GENERATED lint documentation rows — DO NOT EDIT.\n");
    out.push_str("// Regenerate with:\n");
    out.push_str("//   cargo run -p upstream-gen --bin linter-meta-gen -- --sdk-src <dart-sdk-tree> --lints-src <lints-tree>\n");
    out.push_str("// Source: hasPublishedDocs of pkg/linter/messages.yaml (the pinned\n");
    out.push_str("// SDK checkout), one row per first-party linter rule name.\n");
    out.push_str("&[\n");
    for row in rows {
        let _ = writeln!(out, "    (\"{}\", {}),", row.name, row.published);
    }
    out.push_str("]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_follows_the_generated_names() {
        assert_eq!(
            snake_case("alwaysDeclareReturnTypes"),
            "always_declare_return_types"
        );
        assert_eq!(
            snake_case("linesLongerThan80Chars"),
            "lines_longer_than_80_chars"
        );
        assert_eq!(
            snake_case("preferIsEmptyUseIsEmpty"),
            "prefer_is_empty_use_is_empty"
        );
        assert_eq!(
            snake_case("analyzerToBeDeprecatedUse"),
            "analyzer_to_be_deprecated_use"
        );
    }

    #[test]
    fn version_ctor_shapes_parse() {
        assert_eq!(
            parse_version_ctor("Version(3, 11, 0)").as_deref(),
            Some("3.11.0")
        );
        assert_eq!(parse_version_ctor(".new(3, 13, 0)").as_deref(), Some("3.13.0"));
        assert_eq!(parse_version_ctor("dart3_3"), None);
        let consts = version_consts(concat!(
            "final Version dart2_12 = Version(2, 12, 0);\n",
            "final Version dart3 = Version(3, 0, 0);\n",
            "final class RuleState { }\n",
        ));
        assert_eq!(consts["dart3"], "3.0.0");
    }

    #[test]
    fn rule_file_facts_extract_codes_and_since() {
        let multi = concat!(
            "class PreferIsEmpty extends AnalysisRule {\n",
            "  new() : super(name: LintNames.prefer_is_empty);\n",
            "  List<DiagnosticCode> get diagnosticCodes => [\n",
            "    diag.preferIsEmptyAlwaysFalse,\n",
            "    diag.preferIsEmptyUseIsEmpty,\n",
            "  ];\n",
            "}\n",
        );
        let facts = scan_rule_files(&[("prefer_is_empty.dart".to_owned(), multi.to_owned())]);
        assert_eq!(
            facts["prefer_is_empty"].codes,
            ["preferIsEmptyAlwaysFalse", "preferIsEmptyUseIsEmpty"]
        );
        let removed = concat!(
            "final avoidAs = RemovedAnalysisRule(\n",
            "  name: LintNames.avoid_as,\n",
            "  description: _desc,\n",
            "  since: dart2_12,\n",
            ");\n",
        );
        let facts = scan_rule_files(&[("avoid_as.dart".to_owned(), removed.to_owned())]);
        assert_eq!(
            facts["avoid_as"].removed_since,
            Some(Some(SinceArg::Const("dart2_12".to_owned())))
        );
        let unversioned = concat!(
            "final rule = RemovedAnalysisRule(\n",
            "  name: LintNames.avoid_unstable_final_fields,\n",
            ");\n",
        );
        let facts = scan_rule_files(&[("x.dart".to_owned(), unversioned.to_owned())]);
        assert_eq!(
            facts["avoid_unstable_final_fields"].removed_since,
            Some(None)
        );
        let dot_shorthand = concat!(
            "class Rule extends AnalysisRule {\n",
            "  new() : super(\n",
            "        name: LintNames.unnecessary_await_in_return,\n",
            "        state: .deprecated(since: .new(3, 13, 0)),\n",
            "      );\n",
            "}\n",
        );
        let facts = scan_rule_files(&[("y.dart".to_owned(), dot_shorthand.to_owned())]);
        assert_eq!(
            facts["unnecessary_await_in_return"].deprecated_since,
            Some(Some(SinceArg::Version("3.13.0".to_owned())))
        );
    }

    #[test]
    fn lints_set_names_reads_plain_entries() {
        let names = lints_set_names(concat!(
            "linter:\n  rules:\n    - avoid_empty_else\n",
            "    - curly_braces_in_flow_control_structures\n",
        ))
        .expect("parses");
        assert_eq!(
            names,
            ["avoid_empty_else", "curly_braces_in_flow_control_structures"]
        );
        assert!(lints_set_names("linter:\n  rules:\n    avoid_print: true\n").is_err());
    }

    /// The fold on a miniature yaml: name folding, param conversion, the
    /// selection chain (override → base const → file codes → yaml order),
    /// group classification, and the since resolution.
    #[test]
    fn fold_selects_and_classifies() {
        let yaml = concat!(
            "LinterLintCode:\n",
            "  preferIsEmptyAlwaysFalse:\n",
            "    type: lint\n",
            "    parameters:\n",
            "      Object p0: undocumented\n",
            "    sharedName: preferIsEmpty\n",
            "    problemMessage: \"The comparison is always '#p0'.\"\n",
            "    hasPublishedDocs: false\n",
            "  preferIsEmptyUseIsEmpty:\n",
            "    type: lint\n",
            "    parameters:\n",
            "      Object p0: undocumented\n",
            "    sharedName: preferIsEmpty\n",
            "    problemMessage: \"Use '#p0' instead of 'length'.\"\n",
            "    hasPublishedDocs: false\n",
            "  avoidAs:\n",
            "    type: lint\n",
            "    parameters:\n",
            "      Object p0: undocumented\n",
            "    state:\n",
            "      stable: \"2.0\"\n",
            "      removed: \"3.0\"\n",
            "    hasPublishedDocs: false\n",
            "  emptyCatches:\n",
            "    type: lint\n",
            "    parameters:\n",
            "      Object p0: undocumented\n",
            "    problemMessage: 'Avoid empty catches.'\n",
            "    hasPublishedDocs: true\n",
        );
        let entries = parse_messages(yaml).expect("parses");
        let files = [(
            "prefer_is_empty.dart".to_owned(),
            concat!(
                "class R extends AnalysisRule {\n",
                "  new() : super(name: LintNames.prefer_is_empty);\n",
                "  List<DiagnosticCode> get diagnosticCodes => [\n",
                "    diag.preferIsEmptyAlwaysFalse,\n",
                "    diag.preferIsEmptyUseIsEmpty,\n",
                "  ];\n",
                "}\n",
            )
            .to_owned(),
        ), (
            "avoid_as.dart".to_owned(),
            "final avoidAs = RemovedAnalysisRule(\n  name: LintNames.avoid_as,\n  since: dart2_12,\n);\n".to_owned(),
        )];
        let facts = scan_rule_files(&files);
        let versions = version_consts("final Version dart2_12 = Version(2, 12, 0);\n");
        let core = ["empty_catches".to_owned()];
        let rows =
            fold(&entries, &facts, &versions, &core, &[]).expect("fold succeeds");
        assert_eq!(rows.len(), 3);
        // Name-sorted: avoid_as, empty_catches, prefer_is_empty.
        assert_eq!(rows[0].name, "avoid_as");
        assert_eq!(rows[0].group, "removed");
        assert_eq!(rows[0].state, "removed");
        assert_eq!(rows[0].since.as_deref(), Some("2.12.0"));
        assert_eq!(rows[0].problem, "");
        assert_eq!(rows[1].name, "empty_catches");
        assert_eq!(rows[1].group, "core");
        assert_eq!(rows[1].problem, "Avoid empty catches.");
        // The SELECTION_OVERRIDES pin wins over the file's first code.
        assert_eq!(rows[2].name, "prefer_is_empty");
        assert_eq!(rows[2].problem, "Use '{0}' instead of 'length'.");
        assert_eq!(rows[2].group, "standalone");
    }

    #[test]
    fn unresolved_param_reference_fails() {
        let err = convert_refs("uses #nope", &["p0".to_owned()], "some_rule");
        assert!(err.is_err());
        assert_eq!(
            convert_refs("uses #p0 and #p0", &["p0".to_owned()], "r")
                .expect("resolves"),
            "uses {0} and {0}"
        );
    }

    #[test]
    fn renders_the_generated_module_shape() {
        let rows = vec![RuleRow {
            name: "avoid_empty_else".to_owned(),
            group: "core",
            problem: "Empty else block.".to_owned(),
            correction: String::new(),
            state: "stable",
            since: None,
            published: true,
        }];
        let doc = render_rule_meta(&rows, 1, "dart 3.15.0 (main channel)");
        assert!(doc.contains("pub static RULE_META: &[RuleMeta] = TABLE;"));
        assert!(
            doc.contains("RuleMeta { name: \"avoid_empty_else\", group: Group::Core, problem: \"Empty else block.\", correction: \"\", state: MetaState::Stable, docs_published: true },")
        );
        assert!(doc.contains("pub(crate) const fn meta_group"));
        assert!(doc.contains("1 consts fold to 1 name rows"));
        let docs = render_lint_docs_rows(&rows);
        assert!(docs.contains("(\"avoid_empty_else\", true),"));
        assert!(docs.starts_with("// GENERATED"));
    }
}


