// Targets the type-literal family (analyzer ParserErrorCode, pinned
// snapshot syntax-codes.tsv; the analyzer sources are not vendored in
// .third).
// Targets expected_list_or_map_literal (TSV:100): type arguments not
// followed by a list or map literal.
var expectedListOrMap = <int> 3;

// Targets expected_named_type (TSV:101, "Expected a class name."): a
// literal where an identifier (class name) is required. The oracle may
// report expected_type_name for the same shape; both are gate codes.
class ImplementsLiteral implements 3 {}
