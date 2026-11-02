//! The static type of a completed send whose callee carries a DECLARED
//! type rather than a signature: a function-typed field or getter
//! (`comparison(a, b)` on `int Function(E, E) comparison`), a formal of
//! function type, the bare `Function` interface. An invocation's static
//! type is the signature's return (the official
//! `FunctionExpressionInvocation` resolution reads `staticType.returnType`),
//! never the callee's own type — the row keeps the callee's declared type
//! for the member walks, and every result-position read routes through
//! here.

use super::*;

impl Engine<'_> {
    /// The result of invoking a value of declared type `ty`: a
    /// non-generic function type answers its return, the bare `Function`
    /// interface answers `dynamic` (the `Function.apply`-less dispatch), a
    /// generic function type's return depends on inference (`Unknown`).
    /// Every other shape answers `None` — a callable class resolves
    /// through its `call` member elsewhere, and a non-callable declared
    /// type is the receiver of the resolution faces' own verdict.
    pub(crate) fn callable_result_of(&self, ty: Ty) -> Option<Ty> {
        match ty {
            Ty::Function { shape, .. } => {
                let pool = self.boundary.pool();
                let shape = pool.function_shape(shape);
                Some(if shape.type_params == 0 {
                    shape.return_ty
                } else {
                    Ty::UNKNOWN
                })
            }
            Ty::Named { name, .. } if self.strings.resolve(name) == "Function" => Some(Ty::Dynamic),
            _ => None,
        }
    }

    /// The send's result for an `Invoke` row whose stored type is the
    /// callee's declared type; `None` for every read and for a callee
    /// whose declared type is not itself callable.
    pub(crate) fn invoked_callee_result(&self, row: &Row) -> Option<Ty> {
        if row.intent != Intent::Invoke {
            return None;
        }
        self.callable_result_of(row.ty)
    }

    /// The result of invoking a resolved member by name: a method's
    /// declared return; an accessor's or a field's callable result when
    /// its declared type is a function type (an accessor of a non-callable
    /// type keeps its declared type — the `call`-member dispatch the
    /// receiver faces decide); a setter answers nothing.
    pub(crate) fn member_invoke_result(&self, entry: &MemberEntry) -> Ty {
        match entry.kind {
            MemberKind::Method => entry.return_ty,
            MemberKind::Getter => self
                .callable_result_of(entry.return_ty)
                .unwrap_or(entry.return_ty),
            MemberKind::Field => self.callable_result_of(entry.var_ty).unwrap_or(Ty::UNKNOWN),
            MemberKind::Setter => Ty::UNKNOWN,
        }
    }
}
