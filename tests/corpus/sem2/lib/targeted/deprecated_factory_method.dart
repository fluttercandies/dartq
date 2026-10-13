// Targets deprecated_factory_method: below the primary-constructors
// feature (pinned here with a per-file language downgrade, exactly like
// the regression nail), 'factory ()' parses as a method named 'factory'
// and is deprecated because the name becomes a constructor keyword.
// Oracle message (nail-pinned): "Methods named 'factory' will become
// constructors when the primary_constructors feature is enabled."
// Co-fire: body_might_complete_normally (sem2) on the empty-bodied member.
// @dart=3.10
class C {
  external factory ();
  factory () => C();
  factory () {}
}
