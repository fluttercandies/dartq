// Intentional diagnostics: undefined_enum_constant. Two lookups miss the
// closed enum value set; both names are attributed individually.
enum DockStatus { open, closed, maintenance }

String describe(String raw) {
  final first = switch (raw) {
    // expect: undefined_enum_constant
    'holding' => DockStatus.holding,
    _ => DockStatus.open,
  };
  // expect: undefined_enum_constant
  final second = DockStatus.sealedForAudit;
  return '$first$second';
}
