// Targets equal_keys_in_map: a non-const map literal whose later entry's
// key constant-evaluates to a value an earlier key already produced
// reports at the later key (BestPracticesVerifier _checkForDuplications,
// WARNING tier). Probed against the analyzer 3.13.1 oracle: one
// WARNING|STATIC_WARNING row at the later key, message verbatim "Two keys
// in a map literal shouldn't be equal."
var m = {'a': 1, 'a': 2};

// Negative: fully distinct entries never report (probed silent).
var n = {'a': 1, 'b': 2};

// Negative: equal values under distinct keys never report — values are
// not participants, only keys are (probed silent).
var o = {'a': 1, 'b': 1};

// Negative: under an explicit const the duplicate face is the
// COMPILE_TIME_ERROR twin equal_keys_in_const_map, not this warning —
// probed (the twin row fires, no STATIC_WARNING row). The twin sits
// outside this corpus's WARNING universe, so the const shape stays out of
// the file to avoid minting a permanent oracle-only row.
