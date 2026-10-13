// Targets invalid_covariant_modifier_in_primary_constructor: the
// 'covariant' modifier on a primary-constructor declaring parameter must
// pair with 'var' (non-final). Oracle message (nail-pinned): "The
// 'covariant' modifier can only be used on non-final declaring
// parameters."
class C(covariant a) {}

class D(covariant final a) {}
