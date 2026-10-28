use super::{Check, NodeKind, RuleSpec, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/no_duplicate_case_values.dart:37-58`
/// — a `case` whose constant value equals an earlier `case` of the same
/// switch reports at the duplicate expression with both sources spelled
/// into `{0}`/`{1}`. The official visitor processes only legacy members
/// (`if (member is SwitchCase)`, :41); the `AstBuilder` materializes that
/// member shape only when `Feature.patterns` is off (`ast_builder.dart:1295`
/// — patterns shipped in 3.0), so at the default language version every
/// case is a `SwitchPatternCase` and the lint is silent — the official
/// test pins exactly that (all default-version duplicates expect no lint;
/// the reports live in `NoDuplicateCaseValuesTestLanguage219`). The gate
/// here is the same version face at unit granularity (the members of one
/// switch are uniformly one shape): the check runs only with `patterns`
/// off. The official keys the map by the computed constant
/// (`computeConstantValue`, :44); the face here is an in-rule constant
/// folder over the syntax substrate — integer/string/bool/null
/// literals, `+ - * ~/ %` and unary `-`, parentheses (64-bit wrapping
/// integers, Dart `~/` truncation and `%` divisor-sign semantics),
/// descending through `ConstantPattern`/`ParenthesizedExpression` and
/// the `CaseExpression` operand wrapper the fixed parse surface puts
/// around every legacy case operand (the bridge's parse posture is
/// patterns-on at every version, so the operand never sits bare under
/// the `SwitchCase`; the wrapper is found by kind — a compound operand
/// spills its subexpression rows around it — and the report face is the
/// operand token range the `exhaustive_cases` member walk's
/// `head_range` reads, the official `reportAtNode(expression)` anchor).
/// Expressions outside the folder's grammar evaluate unknown and never
/// dedupe — the documented under-report family (const identifiers,
/// doubles, interpolation, adjacent strings, const constructors),
/// mirroring the official `hasKnownValue` skip (:47-49). Message:
/// `diagnostic.g.dart` `noDuplicateCaseValues`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "no_duplicate_case_values",
    &[NodeKind::SwitchStatement],
    check,
);

fn check(c: &mut Check<'_>) {
    // The official `member is SwitchCase` gate (:41): with `Feature.
    // patterns` on (the 3.0+ default) every member is a pattern case and
    // the visitor finds nothing to process.
    if c.unit.features.patterns {
        return;
    }
    let Some(block) = c
        .collect_direct(c.node())
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
    else {
        return;
    };
    // Collect first: each case re-enters `direct_children` (the scratch
    // RefCell forbids a live outer guard).
    let cases: Vec<u32> = c
        .direct_children(block)
        .iter()
        .copied()
        .filter(|&kid| c.kind(kid) == NodeKind::SwitchCase)
        .collect();
    let mut seen: Vec<(ConstValue, core::ops::Range<u32>)> = Vec::new();
    for case in cases {
        // The member's case heads in source order: the real parse merges
        // a `case 1: case 1:` label chain into ONE `SwitchCase` member
        // (payload word 2 counts the heads), each head materializing its
        // `CaseExpression` wrapper row as a direct child — the official
        // shape is one member per head (`member.expression`, singular),
        // so the dedup walk must see every head or a same-member chain
        // never dedupes. The wrapper row is found by kind: a compound
        // operand (`1 + 2`) spills its subexpression rows into the
        // member window around it, so positional extraction lands on a
        // fragment.
        let heads: Vec<u32> = c
            .collect_direct(case)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::CaseExpression)
            .collect();
        for head in heads {
            // The operand's token face (the `head_range` read the member
            // walk shares — the official reports at `expression`). The
            // operand rows tile flat around the childless
            // `ConstantPattern` wrap (`case 1 + 2` keeps its
            // subexpression rows outside it), so the token range — not
            // the row descent — is the fold's substrate.
            let Some(range) = super::exhaustive_cases::head_range(c, head) else {
                continue;
            };
            // `computeConstantValue` → `hasKnownValue` or skip (:44-49).
            let Some(value) = fold_range(c, range.clone()) else {
                continue;
            };
            if let Some((_, original)) = seen.iter().find(|(seen, _)| *seen == value) {
                let dup = token_slice(c, range.clone()).to_owned();
                let orig = token_slice(c, original.clone()).to_owned();
                let span = utf16_token_span(c, range.start).start
                    ..utf16_token_span(c, range.end - 1).end;
                c.report(span, &[&dup, &orig]);
            } else {
                seen.push((value, range));
            }
        }
    }
}

/// The source text of one operand token range — the official
/// `expression.toString()` face the message spells into `{0}`/`{1}`
/// (`'1 + 2'`, exactly as written).
fn token_slice<'a>(c: &'a Check<'_>, range: core::ops::Range<u32>) -> &'a str {
    let Some(first) = c.unit.tokens.get(range.start as usize) else {
        return "";
    };
    let Some(last_index) = range.end.checked_sub(1) else {
        return "";
    };
    let Some(last) = c.unit.tokens.get(last_index as usize) else {
        return "";
    };
    let end = last.byte_offset as usize + c.lexeme(last_index).len();
    &c.unit.source[first.byte_offset as usize..end]
}

/// One computed case value — the map key the official builds from
/// `DartObject`s (:38, :51).
#[derive(PartialEq)]
enum ConstValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(Box<str>),
}

/// The fold's recursion cap — the parser's expression-depth fuse scale
/// (`dartq-parser` `MAX_DEPTH = 500`, the official `parser_impl.dart`
/// :6936 fuse; `pub(crate)` there, so restated). The official
/// `computeConstantValue` walk (`ConstantVisitor`, analyzer
/// `evaluation.dart`) carries no depth bound of its own — an operand
/// nested that deep never reaches it, because the parser fuse recovers
/// the form first. Same posture here: the fold re-walks the raw token
/// span, which post-recovery the AST fuse need not bound, so the walk
/// carries its own fuse. An operand that survived parsing nests strictly
/// under the parser's cap, so every legit prefix run folds below this;
/// reaching it means a recovered/pathological span. Past the cap the
/// value is unknown (the official `hasKnownValue` skip, :47-49) — an
/// internal bound, not a source error, so no diagnostic.
const MAX_FOLD_DEPTH: u32 = 500;

/// The constant fold over one operand's token range: integer, plain
/// string, bool, and null literals; parentheses; unary `-`; and the
/// binary `+ - * ~/ %` at Dart precedence (`* ~/ %` binds over
/// `+ -`), `+` concatenating strings. `None` — outside the grammar,
/// malformed, or past [`MAX_FOLD_DEPTH`]: the official `hasKnownValue`
/// skip (:47-49).
fn fold_range(c: &Check<'_>, range: core::ops::Range<u32>) -> Option<ConstValue> {
    let mut folder = Folder {
        c,
        cursor: range.start,
        end: range.end,
        depth: 0,
    };
    let value = folder.expression(0)?;
    (folder.cursor == folder.end).then_some(value)
}

/// One precedence-climbing pass over the operand range (`min_level`:
/// the lowest binding precedence still folding at this level).
struct Folder<'a, 'b> {
    c: &'a Check<'b>,
    cursor: u32,
    end: u32,
    /// Recursion depth on the unbounded descent edges (unary prefix
    /// runs, parenthesized nesting) — the binary walk is bounded by the
    /// two-row precedence table and needs no fuse.
    depth: u32,
}

impl<'b> Folder<'_, 'b> {
    /// The current lexeme, `None` past the range end.
    fn lexeme(&self) -> Option<&'b str> {
        (self.cursor < self.end).then(|| self.c.lexeme(self.cursor))
    }

    fn expression(&mut self, min_level: u8) -> Option<ConstValue> {
        let mut left = self.unary()?;
        loop {
            let Some(lexeme) = self.lexeme() else {
                return Some(left);
            };
            let level = match lexeme {
                "*" | "~/" | "%" => 2u8,
                "+" | "-" => 1,
                _ => return Some(left),
            };
            if level < min_level {
                return Some(left);
            }
            let operator = lexeme;
            self.cursor += 1;
            let right = self.expression(level + 1)?;
            left = fold_binary(operator, left, right)?;
        }
    }

    /// One recursion level on an unbounded descent edge: past
    /// [`MAX_FOLD_DEPTH`] the fold bails. The counter restores on the
    /// success unwind; a `None` discards the whole folder, so no
    /// restore is owed on a bail.
    fn descend(&mut self) -> Option<()> {
        if self.depth >= MAX_FOLD_DEPTH {
            return None;
        }
        self.depth += 1;
        Some(())
    }

    fn unary(&mut self) -> Option<ConstValue> {
        if self.lexeme()? != "-" {
            return self.primary();
        }
        self.cursor += 1;
        self.descend()?;
        let value = match self.unary()? {
            ConstValue::Int(value) => ConstValue::Int(value.wrapping_neg()),
            _ => return None,
        };
        self.depth -= 1;
        Some(value)
    }

    fn primary(&mut self) -> Option<ConstValue> {
        let token = self.cursor;
        let lexeme = self.lexeme()?;
        self.cursor += 1;
        match lexeme {
            "(" => {
                self.descend()?;
                let value = self.expression(0)?;
                (self.lexeme()? == ")").then_some(())?;
                self.depth -= 1;
                self.cursor += 1;
                Some(value)
            }
            "true" => Some(ConstValue::Bool(true)),
            "false" => Some(ConstValue::Bool(false)),
            "null" => Some(ConstValue::Null),
            _ => match self.c.unit.tokens.get(token as usize)?.kind {
                dartq_scanner::TokenKind::Int => Some(ConstValue::Int(int_literal(lexeme)?)),
                dartq_scanner::TokenKind::String => string_literal(self.c, token),
                _ => None,
            },
        }
    }
}

/// The folder's binary faces: `+` over integers and string
/// concatenation, `- * ~/ %` over integers — 64-bit wrapping like the
/// VM's int, `~/` truncating toward zero, `%` taking the divisor's
/// sign. Division by zero throws officially, so the value is not a
/// known constant.
fn fold_binary(operator: &str, left: ConstValue, right: ConstValue) -> Option<ConstValue> {
    match (operator, left, right) {
        ("+", ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(a.wrapping_add(b)))
        }
        ("+", ConstValue::Str(a), ConstValue::Str(b)) => {
            Some(ConstValue::Str(format!("{a}{b}").into()))
        }
        ("-", ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(a.wrapping_sub(b)))
        }
        ("*", ConstValue::Int(a), ConstValue::Int(b)) => {
            Some(ConstValue::Int(a.wrapping_mul(b)))
        }
        ("~/", ConstValue::Int(a), ConstValue::Int(b)) if b != 0 => {
            Some(ConstValue::Int(a.wrapping_div(b)))
        }
        ("%", ConstValue::Int(a), ConstValue::Int(b)) if b != 0 => {
            let mut rem = a.wrapping_rem(b);
            if rem != 0 && (rem < 0) != (b < 0) {
                rem = rem.wrapping_add(b);
            }
            Some(ConstValue::Int(rem))
        }
        _ => None,
    }
}

/// The Dart integer literal grammar the folder reads — decimal and hex
/// (`0x`), digit separators dropped, wrapping to the VM's 64-bit int.
fn int_literal(lexeme: &str) -> Option<i64> {
    let text: String = lexeme.chars().filter(|&ch| ch != '_').collect();
    let (digits, radix) = match text.get(..2) {
        Some("0x" | "0X") => (&text[2..], 16),
        _ => (&text[..], 10),
    };
    u64::from_str_radix(digits, radix)
        .ok()
        .map(u64::cast_signed)
}

/// The value of one string-literal token: quote forms `'…'` / `"…"` and
/// the triple variants, `r`-raw bodies verbatim, cooked bodies
/// escape-decoded. A `$` in a cooked body is interpolation (a
/// token-layer face) — unknown.
fn string_literal(c: &Check<'_>, token: u32) -> Option<ConstValue> {
    let text = c.lexeme(token);
    let (raw, text) = match text.strip_prefix('r') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let body = if let Some(rest) = text.strip_prefix("'''") {
        rest.strip_suffix("'''")?
    } else if let Some(rest) = text.strip_prefix("\"\"\"") {
        rest.strip_suffix("\"\"\"")?
    } else {
        let quote = text.chars().next().filter(|&ch| ch == '\'' || ch == '"')?;
        text.strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))?
    };
    if raw {
        return Some(ConstValue::Str(body.into()));
    }
    if body.contains('$') {
        return None;
    }
    let mut value = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            value.push(ch);
            continue;
        }
        match chars.next()? {
            'n' => value.push('\n'),
            'r' => value.push('\r'),
            't' => value.push('\t'),
            'b' => value.push('\u{0008}'),
            'f' => value.push('\u{000C}'),
            'v' => value.push('\u{000B}'),
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            'u' => {
                let mut hex = String::new();
                let first = chars.next()?;
                if first == '{' {
                    for ch in chars.by_ref() {
                        if ch == '}' {
                            break;
                        }
                        hex.push(ch);
                    }
                } else {
                    // The peeked character is the first of four hex
                    // digits.
                    hex.push(first);
                    hex.extend(chars.by_ref().take(3));
                }
                value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            escaped @ ('\\' | '\'' | '"') => value.push(escaped),
            _ => return None,
        }
    }
    Some(ConstValue::Str(value.into()))
}
