// Targets conflicting_modifiers: members cannot be declared to be both
// 'late' and 'const'.
class C {
  late const int x = 1;
}
