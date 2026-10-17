// Trigger for dart_code_linter `avoid-shrink-wrap-in-lists`.
// Shape: `shrinkWrap: true` on a scrollable list — the rule's flagged form.
final list = ListView(shrinkWrap: true);

class ListView {
  const ListView({this.shrinkWrap = false});

  final bool shrinkWrap;
}
