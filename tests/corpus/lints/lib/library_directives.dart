// library-directive family — fixture wave 2 kitchen file. The named
// library directive is the unnecessary_library_name face (the
// unnamed_libraries feature shipped in 2.19); the part directive must
// precede declarations, and its target carries the use_string_in_part_of_
// directives row. The @pragma('dart2js:late:trust') annotation is the
// library_annotations face dartq ports (the isPragmaLateTrust slice —
// pragma carries no @Target, so the annotation reports on a declaration
// on both sides; the library directive itself is never checked).
// Expected rows: 2, keyed library_directives.dart —
// UNNECESSARY_LIBRARY_NAME (the library name), LIBRARY_ANNOTATIONS (the
// pragma annotation on LateTrustSite).
library lint_corpus.part_host;

part 'use_string_in_part_of_target.dart';

@pragma('dart2js:late:trust')
class LateTrustSite {}
