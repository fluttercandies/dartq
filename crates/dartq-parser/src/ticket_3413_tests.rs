//! #3413 — anonymous-method selector shapes (`..=>`, `.=>`, `?..=>`).
//!
//! The conformance family `features/anonymous-methods` reports 85 EXTRA
//! `undefined_*` rows (61 `undefined_identifier`, 18 `undefined_getter`,
//! 6 `undefined_function`; the ticket's snapshot counted 74). Every extra row
//! lands inside an anonymous-method BODY: `buffer.=> length` reports
//! `Undefined name 'length'`, `buffer.=> this.length` reports
//! `invalid_reference_to_this` (or `The getter 'length' isn't defined for the
//! type '<enclosing>'` when the enclosing container has a `this`), i.e. the
//! body resolves against the ENCLOSING scope instead of binding `this` to the
//! receiver. Upstream binds it in the resolver, not the parser
//! (`pkg/analyzer/lib/src/generated/resolver.dart:1952-2087`
//! `visitAnonymousMethodInvocation`: `_thisType = parameterType` around
//! `body.resolve(...)`, `thisBinding_begin(targetInfo)` for flow, and the
//! receiver's type injected into the first unannotated formal).
//!
//! These pins settle the parser half of the question with evidence: the
//! selector node IS built, with upstream's exact event shape and spans
//! (`pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:7550-7590`
//! `_parseAnonymousMethod`, `:7292-7295` selector dispatch, `:7770-7801`
//! cascade dispatch). A downstream builder therefore receives everything the
//! official `AstBuilder.endAnonymousMethodInvocation`
//! (`pkg/analyzer/lib/src/fasta/ast_builder.dart:906-950`) pops: the receiver
//! was pushed before the begin event, the punctuation span is the member
//! operator itself, the body expression fires between begin and end, and
//! `endAnonymousMethodInvocation` carries the `=>` and the body's last token.

use crate::ParserFeatures;
use crate::listener::{IdentifierContext, ParserListener, Token};
use crate::tests::collector::scan_ok;
use dartq_language::{Feature, FeatureSet};

/// The latest fold with the anonymous-methods experiment opted in — the
/// corpus `analyzer: enable-experiment: [anonymous-methods]` channel folds
/// through the same set.
fn anon_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(
        FeatureSet::latest().with_experiment(Feature::AnonymousMethods),
    )
}

/// `(UTF-16 offset, UTF-16 length)` of a span — the shape facts these pins
/// compare.
fn r(span: Token) -> (u32, u32) {
    (span.range.offset, span.range.length)
}

/// One recorded event. Identifiers and the member events are interleaved in
/// fire order, which is exactly what the receiver/body stack discipline of a
/// downstream builder depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ev {
    Ident(u32, u32),
    BeginAnon(u32, u32),
    ImplicitParams(u32, u32),
    EndAnon {
        punct: (u32, u32),
        function_definition: Option<(u32, u32)>,
        end: (u32, u32),
        is_expression: bool,
    },
    BeginCascade(u32, u32),
    CascadeAccess {
        operator: (u32, u32),
        member: (u32, u32),
        null_aware: bool,
    },
    DotAccess {
        operator: (u32, u32),
        member: (u32, u32),
        null_aware: bool,
    },
}

/// Records only the events the selector-shape question is about.
#[derive(Default)]
struct ShapeProbe {
    events: Vec<Ev>,
}

impl ParserListener for ShapeProbe {
    fn handle_identifier(&mut self, token: Token, _context: IdentifierContext) {
        let (offset, length) = r(token);
        self.events.push(Ev::Ident(offset, length));
    }

    fn begin_anonymous_method_invocation(&mut self, token: Token) {
        let (offset, length) = r(token);
        self.events.push(Ev::BeginAnon(offset, length));
    }

    fn handle_implicit_formal_parameters(&mut self, token: Token) {
        let (offset, length) = r(token);
        self.events.push(Ev::ImplicitParams(offset, length));
    }

    fn end_anonymous_method_invocation(
        &mut self,
        begin: Token,
        function_definition: Option<Token>,
        end: Token,
        is_expression: bool,
    ) {
        self.events.push(Ev::EndAnon {
            punct: r(begin),
            function_definition: function_definition.map(r),
            end: r(end),
            is_expression,
        });
    }

    fn begin_cascade(&mut self, operator: Token) {
        let (offset, length) = r(operator);
        self.events.push(Ev::BeginCascade(offset, length));
    }

    fn handle_cascade_access(&mut self, operator: Token, end: Token, is_null_aware: bool) {
        self.events.push(Ev::CascadeAccess {
            operator: r(operator),
            member: r(end),
            null_aware: is_null_aware,
        });
    }

    fn handle_dot_access(&mut self, token: Token, end: Token, is_null_aware: bool) {
        self.events.push(Ev::DotAccess {
            operator: r(token),
            member: r(end),
            null_aware: is_null_aware,
        });
    }
}

/// Parses `source` with the given feature fold and returns the recorded
/// selector shape events.
fn shape(source: &'static str, features: ParserFeatures) -> Vec<Ev> {
    let (table, text) = scan_ok(source);
    let mut probe = ShapeProbe::default();
    let mut parser = crate::Parser::new(text, table, features, &mut probe);
    parser.parse_unit();
    parser.listener.events.clone()
}

/// Index of the single event matching `want`, asserting there is exactly one.
fn only(events: &[Ev], want: impl Fn(&Ev) -> bool) -> usize {
    let hits: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, ev)| want(ev))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one match, events: {events:?}"
    );
    hits[0]
}

/// Asserts the anonymous-member shape shared by all three spellings: the
/// receiver identifier lands BEFORE the begin event, the implicit-parameter
/// event repeats the punctuation, the body identifier lands between begin and
/// end, and the end event carries the `=>` plus the body's send-leaf end — no
/// dot-access/cascade-access event for the member (the member events carry it
/// instead, `parser_impl.dart:7575-7590`).
///
/// `body_end` is the body expression's send-leaf end token. For a bare name
/// that is the unconsumed FOLLOWER (`;`) — dartq's documented send-leaf span
/// tiling ([`crate::expressions`] `parse_send_from_identifier`, #585), the same
/// span the body's own `handle_send` end carries.
fn assert_anon_member_shape(
    events: &[Ev],
    receiver: (u32, u32),
    punctuation: (u32, u32),
    arrow: (u32, u32),
    body: (u32, u32),
    body_end: (u32, u32),
) {
    let begin = only(events, |ev| matches!(ev, Ev::BeginAnon(..)));
    assert_eq!(events[begin], Ev::BeginAnon(punctuation.0, punctuation.1));
    assert_eq!(
        events[begin + 1],
        Ev::ImplicitParams(punctuation.0, punctuation.1),
        "the implicit-formal-parameters event repeats the punctuation \
         (`handleImplicitFormalParameters(punctuation)`, parser_impl.dart:7564)"
    );
    let end = only(events, |ev| matches!(ev, Ev::EndAnon { .. }));
    assert_eq!(
        events[end],
        Ev::EndAnon {
            punct: punctuation,
            function_definition: Some(arrow),
            end: body_end,
            is_expression: true,
        }
    );
    assert!(
        events
            .iter()
            .any(|ev| *ev == Ev::Ident(receiver.0, receiver.1)),
        "the receiver must be pushed before the member opens \
         (`AstBuilder.endAnonymousMethodInvocation` pops it, ast_builder.dart:921): {events:?}"
    );
    let receiver_at = events
        .iter()
        .position(|ev| *ev == Ev::Ident(receiver.0, receiver.1))
        .expect("receiver identifier");
    let body_at = events
        .iter()
        .position(|ev| *ev == Ev::Ident(body.0, body.1))
        .expect("body identifier");
    assert!(receiver_at < begin, "receiver before begin: {events:?}");
    assert!(
        begin < body_at && body_at < end,
        "body between begin and end: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|ev| matches!(ev, Ev::DotAccess { .. } | Ev::CascadeAccess { .. })),
        "the anonymous arm fires no dot/cascade access event: {events:?}"
    );
}

/// `buffer.=> foo` — the selector face. `parser_impl.dart:7292-7295` opens
/// `_parseAnonymousMethod(dot, afterDot)` with the `.` as the punctuation.
#[test]
fn selector_arrow_punctuation_is_the_dot() {
    let events = shape("void main() {\n  buffer.=> foo;\n}\n", anon_features());
    // `buffer` 16..22, `.` 22..23, `=>` 23..25, `foo` 26..29, `;` 29..30.
    assert_anon_member_shape(&events, (16, 6), (22, 1), (23, 2), (26, 3), (29, 1));
}

/// `buffer..=> foo` — the cascade initial-section face
/// (`parser_impl.dart:7766-7774`): `beginCascade` first, then the member
/// opens with the WHOLE `..` operator as its punctuation, and the section
/// fires no `handleCascadeAccess`.
#[test]
fn cascade_arrow_punctuation_is_the_double_dot() {
    let events = shape("void main() {\n  buffer..=> foo;\n}\n", anon_features());
    // `buffer` 16..22, `..` 22..24, `=>` 24..26, `foo` 27..30, `;` 30..31.
    assert_anon_member_shape(&events, (16, 6), (22, 2), (24, 2), (27, 3), (30, 1));
    let cascade_at = only(&events, |ev| matches!(ev, Ev::BeginCascade(..)));
    assert_eq!(events[cascade_at], Ev::BeginCascade(22, 2));
    let begin_at = only(&events, |ev| matches!(ev, Ev::BeginAnon(..)));
    assert!(cascade_at < begin_at, "cascade opens before its section");
}

/// `buffer?..=> foo` — the null-aware cascade face (`parser_impl.dart:7771`):
/// the punctuation is the single `?..` token (three UTF-16 units), and the
/// member fires no `handleNullAwareMemberBegin` split (the anonymous arm
/// replaces the null-aware member events).
#[test]
fn null_aware_cascade_arrow_punctuation_is_the_question_double_dot() {
    let events = shape("void main() {\n  buffer?..=> foo;\n}\n", anon_features());
    // `buffer` 16..22, `?..` 22..25, `=>` 25..27, `foo` 28..31, `;` 31..32.
    assert_anon_member_shape(&events, (16, 6), (22, 3), (25, 2), (28, 3), (31, 1));
    let cascade_at = only(&events, |ev| matches!(ev, Ev::BeginCascade(..)));
    assert_eq!(events[cascade_at], Ev::BeginCascade(22, 3));
}

/// Negative control: an ordinary cascade section still fires
/// `beginCascade` + `handleCascadeAccess` (`parser_impl.dart:7775-7786`) and
/// never an anonymous-method event.
#[test]
fn ordinary_cascade_section_stays_a_cascade_access() {
    let events = shape("void main() {\n  a..b;\n}\n", anon_features());
    assert!(
        !events.iter().any(|ev| matches!(
            ev,
            Ev::BeginAnon(..) | Ev::EndAnon { .. } | Ev::ImplicitParams(..)
        )),
        "no anonymous-method events for `a..b`: {events:?}"
    );
    // `a` 16..17, `..` 17..19, `b` 19..20, `;` 20..21 (the member's
    // send-leaf end is the unconsumed follower, #585).
    assert!(events.contains(&Ev::BeginCascade(17, 2)), "{events:?}");
    assert!(
        events.contains(&Ev::CascadeAccess {
            operator: (17, 2),
            member: (20, 1),
            null_aware: false,
        }),
        "{events:?}"
    );
}

/// Negative control: an ordinary property access still fires
/// `handleDotAccess` and never an anonymous-method event.
#[test]
fn ordinary_dot_access_stays_a_dot_access() {
    let events = shape("void main() {\n  a.foo;\n}\n", anon_features());
    assert!(
        !events.iter().any(|ev| matches!(
            ev,
            Ev::BeginAnon(..) | Ev::EndAnon { .. } | Ev::ImplicitParams(..)
        )),
        "no anonymous-method events for `a.foo`: {events:?}"
    );
    // `a` 16..17, `.` 17..18, `foo` 18..21, `;` 21..22 (the access's
    // send-leaf end is the unconsumed follower, #585).
    assert_eq!(
        events
            .iter()
            .filter(|ev| matches!(ev, Ev::DotAccess { .. }))
            .cloned()
            .collect::<Vec<_>>(),
        vec![Ev::DotAccess {
            operator: (17, 1),
            member: (21, 1),
            null_aware: false,
        }],
        "{events:?}"
    );
}
