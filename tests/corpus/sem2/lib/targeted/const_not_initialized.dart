// Targets const_not_initialized: a const variable with no initializer.
// Oracle message (nail-pinned): "The constant 'c' must be initialized."
// Dartq-side note: the regression nails record dartq emitting the sibling
// code const_field_without_initializer at identical positions where the
// oracle emits const_not_initialized; this fixture triggers the family on
// both sides so the rename is visible to the sem2 tier.
const int c;
