// Trigger for solid_lints `avoid_late_keyword`.
// Shape: `late` variables where initialization could be non-lazy.
late String eagerButDeclaredLate = compute();

String compute() => 'value';

void use() => print(eagerButDeclaredLate);
