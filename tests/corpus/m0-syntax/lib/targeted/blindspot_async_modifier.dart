// Targets the async-modifier family.
// Targets invalid_sync (syntax-codes.tsv:180): 'sync' must be followed by
// '*'. Fasta emit site: pkg/_fe_analyzer_shared/lib/src/parser/
// parser_impl.dart:6697 (invalidSyncModifier).
void syncWithoutStar() sync {}

// Targets invalid_star_after_async (TSV:178): 'async*' isn't allowed for
// an expression ('=>') function body.
var starAfterAsync = () async* null;
