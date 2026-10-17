// comment_references — differential corpus (ticket #1160).
// Source: upstream Dart SDK source, pkg/linter/test/rules/comment_references_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 9 POSITIVE cases (1 row each) + 28 NEGATIVE cases = 37
// upstream tests, none skipped. Upstream cases each compiled standalone;
// declarations here carry block-suffixed names (the rule judges doc-comment
// references by resolution, so renames are face-neutral).
// Expected rows (oracle leg): 9 — COMMENT_REFERENCES keyed
// comment_references.dart, one per POSITIVE block below.

// POSITIVE upstream test_false: [false] does not resolve.
/// [false]
class CFalse {}

// POSITIVE upstream test_null: [null] does not resolve.
/// [null]
class CNull {}

// POSITIVE upstream test_this: [this] does not resolve.
/// [this]
class CThis {}

// POSITIVE upstream test_true: [true] does not resolve.
/// [true]
class CTrue {}

// POSITIVE upstream test_parameter_constructor_privateNamed_invalid: the
// doc reference [x] cannot see the private named parameter (upstream
// issue 62768 context — a lint IS expected here).
class CPrivateNamedInvalid {
  final int _x;

  /// [x]
  CPrivateNamedInvalid({required this._x});
}

// POSITIVE upstream test_unknownElement: [y] resolves to nothing.
/// Text [y].
class CUnknown {}

// POSITIVE upstream test_unknownElement_dottedName: [y.z] resolves to
// nothing.
/// Parameter [y.z].
class CUnknownDotted {}

// POSITIVE upstream test_unknownElement_followedByColon: [y] before a colon
// still reports.
/// Parameter [y]: z.
void fUnknownFollowedByColon(int x) {}

// POSITIVE upstream test_unknownElement_twiceDottedName: [x.y.z] resolves
// to nothing.
/// Parameter [x.y.z].
class CUnknownTwiceDotted {}

// NEGATIVE upstream test_field: [x] resolves to the enclosing field.
class CField {
  int x = 0;
  /// Assigns to [x].
  void m() {}
}

// NEGATIVE upstream test_importedElement: [String] is a core type.
/// [String] is OK.
class CImported {}

// NEGATIVE upstream test_markdown_inlineLink: a markdown inline link is not
// a code reference.
/// A link to [Sha256](http://tools.ietf.org/html/rfc6234) hash function.
class CInlineLink {}

// NEGATIVE upstream test_markdown_inlineLink_onePeriod.
/// A link to [Sha256.Sha256](http://tools.ietf.org/html/rfc6234) hash function.
class CInlineLinkOnePeriod {}

// NEGATIVE upstream test_markdown_inlineLink_textSpansLines.
/// A [link spanning
/// multiple lines](http://tools.ietf.org/html/rfc6234)
class CInlineLinkTextSpansLines {}

// NEGATIVE upstream test_markdown_inlineLink_twoPeriods.
/// A link to [Sha256.Sha256.Sha256](http://tools.ietf.org/html/rfc6234) hash function.
class CInlineLinkTwoPeriods {}

// NEGATIVE upstream test_markdown_inlineLink_withTitle.
/// A link to [Sha256](http://tools.ietf.org/html/rfc6234 "Some") hash function.
class CInlineLinkWithTitle {}

// NEGATIVE upstream test_markdown_referenceLink.
/// A link to [Sha256][rfc] hash function.
///
/// [rfc]: http://tools.ietf.org/html/rfc6234
class CReferenceLink {}

// NEGATIVE upstream test_markdown_referenceLink_onePeriod.
/// A link to [Sha256.Sha256][rfc] hash function.
///
/// [rfc]: http://tools.ietf.org/html/rfc6234
class CReferenceLinkOnePeriod {}

// NEGATIVE upstream test_markdown_referenceLink_shortcut.
/// A link to [rfc][] hash function.
///
/// [rfc]: http://tools.ietf.org/html/rfc6234
class CReferenceLinkShortcut {}

// NEGATIVE upstream test_markdown_referenceLink_shortcut_onePeriod.
/// A link to [rfc.rfc][] hash function.
///
/// [rfc.rfc]: http://tools.ietf.org/html/rfc6234
class CReferenceLinkShortcutOnePeriod {}

// NEGATIVE upstream test_markdown_referenceLink_shortcut_twoPeriods.
/// A link to [rfc.rfc.rfc][] hash function.
///
/// [rfc.rfc.rfc]: http://tools.ietf.org/html/rfc6234
class CReferenceLinkShortcutTwoPeriods {}

// NEGATIVE upstream test_markdown_referenceLink_twoPeriods.
/// A link to [Sha256.Sha256.Sha256][rfc] hash function.
///
/// [rfc]: http://tools.ietf.org/html/rfc6234
class CReferenceLinkTwoPeriods {}

// NEGATIVE upstream test_parameter: [x] resolves to the parameter.
class CParameter {
  /// Reads [x].
  void m(int x) {}
}

// NEGATIVE upstream test_parameter_constructor_field: [x] resolves to the
// initializing formal's field.
class CParameterConstructorField {
  final int x;

  /// [x]
  CParameterConstructorField(this.x);
}

// NEGATIVE upstream test_parameter_constructor_privateNamed_valid: [_x]
// resolves to the private field.
class CPrivateNamedValid {
  final int _x;

  /// [_x]
  CPrivateNamedValid({required this._x});
}

// NEGATIVE upstream test_parameter_constructor_super: [x] resolves through
// the super parameter.
class CParameterSuperBase {
  CParameterSuperBase(int x);
}

class CParameterSuper extends CParameterSuperBase {
  /// [x]
  CParameterSuper(super.x);
}

// NEGATIVE upstream test_prefixedIdentifier_constructorTearoff:
// [Future.delayed] resolves.
/// Text [Future.delayed].
class CPrefixedTearoff {}

// NEGATIVE upstream test_prefixedIdentifier_importPrefix: the prefix
// resolves through the import.
import 'dart:async' as async;
/// Text [async.FutureOr].
class CPrefixedImport {}

// NEGATIVE upstream test_prefixedIdentifier_instanceMember: [int.isEven]
// resolves as an instance member face.
/// Text [int.isEven].
class CPrefixedInstance {}

// NEGATIVE upstream test_prefixedIdentifier_instanceMember_onTypedef.
typedef Int2 = int;

/// Text [Int2.isEven].
class CPrefixedInstanceTypedef {}

// NEGATIVE upstream test_prefixedIdentifier_staticMethod: [Future.wait].
/// Text [Future.wait].
class CPrefixedStaticMethod {}

// NEGATIVE upstream test_prefixedIdentifier_staticProperty: [double.nan].
/// Text [double.nan].
class CPrefixedStaticProperty {}

// NEGATIVE upstream test_propertyAccess: [async.Future.value].
/// Text [async.Future.value].
class CPropertyAccess {}

// NEGATIVE upstream test_propertyAccess_onTypedef: [self.B.x] through the
// self import and a typedef.
import '' as self;
class CSelfTypedefBase {
  int get x => 7;
}
typedef B = CSelfTypedefBase;

/// Text [self.B.x].
class CPropertyAccessTypedef {}

// NEGATIVE upstream test_typeAlias: [Td] resolves to the typedef.
/// Text [Td].
class CTypeAlias {}

typedef Td = CTypeAlias;

// NEGATIVE upstream test_typeName: [Future] resolves.
/// Text [Future].
class CTypeName {}

// NEGATIVE upstream test_unclosedSquareBracket: an unclosed `[` must not
// crash (#819).
/// [
/// ^--- Should not crash (#819).
class CUnclosed {}
