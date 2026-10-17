// Baseline: within-file duplicate, standalone library (not a part).
//
// The per-file duplicate face is already implemented on the dartq side
// (the collector's duplicates log); this file separates that known,
// expected-agreeing row from the cross-part rows the probe measures. If
// this file does not show exactly one `duplicate_definition` row at the
// second `dupName`, the probe machinery itself is in question.
int dupName = 1;

String dupName = 'x';

int wUnique = 2;
