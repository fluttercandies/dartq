// Targets duplicate_export: the same library exported twice by one unit
// reports at the repeat's URI (ImportsVerifier duplicate pair walk, WARNING
// tier).
export 'dart:math';

export 'dart:math';
