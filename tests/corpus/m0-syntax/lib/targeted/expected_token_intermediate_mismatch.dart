// Targets expected_token (intermediate mismatch): a closer matching an outer
// opener pops only the inner level — expected_token pinned at the '}' that
// fails to close '['.
var m = {[1};
