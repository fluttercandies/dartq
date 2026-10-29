use super::{Check, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_js_rounded_ints.dart:24-41`
/// — an integer literal whose value does not survive the f64 round
/// trip (`value.toDouble().toInt() != value`, :29) reports at the
/// literal. Hexadecimal literals wrap modulo 2^64 (the VM face).
pub(crate) static SPEC: RuleSpec =
    crate::spec!("avoid_js_rounded_ints", &[NodeKind::LiteralInt], check,);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(value) = literal_value(c.lexeme(c.head_token(node))) else {
        return;
    };
    // Rust's `as` saturates exactly like the VM's `toInt` clamp on the
    // round trip.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the official rule is defined by the f64 round trip; \
                      both casts are the semantics"
    )]
    if (value as f64) as i64 != value {
        c.report(crate::rules::utf16_token_span(c, c.head_token(node)), &[]);
    }
}

/// The literal's value: digit separators stripped, hexadecimal wraps,
/// decimal beyond `i64::MAX` is unrepresentable (the official
/// `node.value` never materializes there).
fn literal_value(lexeme: &str) -> Option<i64> {
    let digits: String = lexeme.chars().filter(|&ch| ch != '_').collect();
    let hex = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"));
    if let Some(hex) = hex {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "hexadecimal literals wrap modulo 2^64 (the VM face), \
                          the official wrap semantics"
        )]
        return u64::from_str_radix(hex, 16).ok().map(|v| v as i64);
    }
    let value = digits.parse::<u64>().ok()?;
    i64::try_from(value).ok()
}
