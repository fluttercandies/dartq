// Targets the native family (TSV:242/243/244). SDK-gated: the analyzer
// emits these only when the SDK-provenance check fails ("Native clause can
// only be used in the SDK and code that is loaded through native
// extensions", syntax-codes.tsv:242). CFE-side, the deprecated form itself
// is tagged nativeClauseShouldBeAnnotation ("Native clause in this form is
// deprecated.", pkg/_fe_analyzer_shared/lib/src/messages/diagnostic.g.dart)
// and pkg/_fe_analyzer_shared/lib/src/parser/parser.dart:68 notes CFE
// parser tests currently ignore it. Kept in the corpus for gate exposure;
// if the oracle suppresses these outside the SDK, the codes stay
// documented gaps.

// Targets native_clause_in_non_sdk_code (TSV:242) and
// native_clause_should_be_annotation (TSV:243).
class NativeClass native "corpus" {}

// Targets native_function_body_in_non_sdk_code (TSV:244).
void nativeFunction() native "corpus";
