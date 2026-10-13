// Targets final_not_initialized: a final variable with no initializer.
// Oracle message (nail-pinned): "The final variable 'f' must be
// initialized." Dartq-side note: the regression nails record dartq
// emitting the sibling code final_field_without_initializer at identical
// positions where the oracle emits final_not_initialized; this fixture
// triggers the family on both sides so the rename is visible to the sem2
// tier.
final int f;
