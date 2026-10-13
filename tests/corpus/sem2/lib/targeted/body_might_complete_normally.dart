// Targets body_might_complete_normally: a function with a non-nullable
// return type whose body can complete normally. Oracle message
// (nail-pinned): "The body might complete normally, causing 'null' to be
// returned, but the return type, 'int', is a potentially non-nullable
// type."
int f() {}
