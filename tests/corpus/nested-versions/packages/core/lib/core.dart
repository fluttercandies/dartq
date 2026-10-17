// Core member at language version 3.6 (from `sdk: ^3.6.0`): digit
// separators are legal (3.6 positive control), records and switch
// expressions with object patterns are legal (>= 3.0). The 3.7 wildcard
// flip does NOT apply here — see lib/wildcard_repeat.dart for the
// below-version error face.
//
// Expectation: analyzes clean at 3.6 (root options set applies: every
// declaration is typed).

const int kCoreLimit = 1_000_000;

enum Format { plain, tagged }

String describeCore(int limit) {
  final record = (limit: limit, format: Format.tagged);
  return switch (record) {
    (limit: 0, format: _) => 'empty',
    (limit: final l, format: Format.tagged) => 'tagged $l',
    _ => 'plain $limit',
  };
}
