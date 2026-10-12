// Targets equal_elements_in_set: a non-const set literal whose later
// top-level element constant-evaluates to a value an earlier element
// already produced reports at the later element (BestPracticesVerifier
// _checkForDuplications, WARNING tier). Probed against the analyzer 3.13.1
// oracle: one WARNING|STATIC_WARNING row at the later element, message
// verbatim "Two elements in a set literal shouldn't be equal."
var s = {1, 1};

// Negative: distinct elements never report (probed silent).
var t = {1, 2};

// Negative: ints and doubles never unify — 1 and 1.0 are distinct set
// values (probed silent).
var u = {1, 1.0};

// Negative: under an explicit const the duplicate face is the
// COMPILE_TIME_ERROR twin equal_elements_in_const_set, not this warning —
// probed (the twin row fires, no STATIC_WARNING row). The twin sits
// outside this corpus's WARNING universe, so the const shape stays out of
// the file to avoid minting a permanent oracle-only row.
