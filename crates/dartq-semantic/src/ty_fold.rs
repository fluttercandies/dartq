//! Structural traversal over [`Ty`] — the one place that knows the shape
//! of every variant (the `TypeFoldable` / `TypeVisitable` split): type
//! substitution, free-parameter collection, and every future shape-
//! preserving rewrite descend through here instead of re-enumerating the
//! variants at each use site. A named type's argument list, a function
//! type's return and formal columns, and a record type's fields all count
//! as children; the atoms (`dynamic`, `void`, `Never`, `Null`, `Unknown`)
//! have none.
//!
//! A generic function shape (`T Function<T>(T)`) is an opaque leaf for
//! both walks: its own parameters are nameless in the interned shape and
//! may shadow an outer spelling, so nothing inside it is rewritten or
//! collected — the under-report direction for the rare generic-callable
//! formal, never a wrong substitution.

use crate::model::{ArgListId, ArgPool, Ty};
use dartq_source::StringId;

impl ArgPool {
    /// Rewrites every argument-less named leaf of `ty` through `leaf`
    /// (type parameters live there) and re-interns each compound shape
    /// bottom-up — only when a child actually changed, so an untouched
    /// type keeps its identity and the pool grows by nothing.
    pub fn map_leaves(&mut self, ty: Ty, leaf: &mut dyn FnMut(Ty) -> Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } if args != ArgListId::EMPTY => {
                let old = self.get(args).to_vec();
                let new: Vec<Ty> = old.iter().map(|arg| self.map_leaves(*arg, leaf)).collect();
                if new == old {
                    ty
                } else {
                    self.named(name, nullable, &new)
                }
            }
            Ty::Named { .. } => leaf(ty),
            Ty::Function { shape, nullable } => {
                let (return_ty, positional, named) = {
                    let fn_shape = self.function_shape(shape);
                    if fn_shape.type_params != 0 {
                        return ty;
                    }
                    (
                        fn_shape.return_ty,
                        fn_shape.positional.to_vec(),
                        fn_shape.named.to_vec(),
                    )
                };
                let new_return = self.map_leaves(return_ty, leaf);
                let new_positional: Vec<Ty> = positional
                    .iter()
                    .map(|formal| self.map_leaves(*formal, leaf))
                    .collect();
                let new_named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, formal)| (*name, self.map_leaves(*formal, leaf)))
                    .collect();
                if new_return == return_ty && new_positional == positional && new_named == named {
                    ty
                } else {
                    self.function(0, new_return, &new_positional, &new_named, nullable)
                }
            }
            Ty::Record { shape, nullable } => {
                let (positional, named) = {
                    let record = self.record_shape(shape);
                    (record.positional.to_vec(), record.named.to_vec())
                };
                let new_positional: Vec<Ty> = positional
                    .iter()
                    .map(|field| self.map_leaves(*field, leaf))
                    .collect();
                let new_named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, field)| (*name, self.map_leaves(*field, leaf)))
                    .collect();
                if new_positional == positional && new_named == named {
                    ty
                } else {
                    self.record(&new_positional, &new_named, nullable)
                }
            }
            Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Never | Ty::Void => ty,
        }
    }

    /// Visits every named node of `ty` in pre-order — a head before its
    /// arguments — descending function and record shapes alike.
    pub fn visit_named(&self, ty: Ty, visit: &mut dyn FnMut(Ty)) {
        match ty {
            Ty::Named { args, .. } => {
                visit(ty);
                if args != ArgListId::EMPTY {
                    for arg in self.get(args) {
                        self.visit_named(*arg, visit);
                    }
                }
            }
            Ty::Function { shape, .. } => {
                let fn_shape = self.function_shape(shape);
                if fn_shape.type_params != 0 {
                    return;
                }
                self.visit_named(fn_shape.return_ty, visit);
                for formal in &fn_shape.positional {
                    self.visit_named(*formal, visit);
                }
                for (_, formal) in &fn_shape.named {
                    self.visit_named(*formal, visit);
                }
            }
            Ty::Record { shape, .. } => {
                let record = self.record_shape(shape);
                for field in &record.positional {
                    self.visit_named(*field, visit);
                }
                for (_, field) in &record.named {
                    self.visit_named(*field, visit);
                }
            }
            Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Never | Ty::Void => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::StringTable;

    fn rig() -> (ArgPool, StringTable) {
        (ArgPool::default(), StringTable::default())
    }

    #[test]
    fn substitutes_a_parameter_nested_in_a_function_typed_formal() {
        let (mut pool, strings) = rig();
        let (e, string, bool_) = (
            strings.intern("E"),
            strings.intern("String"),
            strings.intern("bool"),
        );
        let formal = pool.function(0, Ty::named(bool_), &[Ty::named(e)], &[], false);
        let member = pool.function(0, Ty::Void, &[formal], &[], false);
        let want_formal = pool.function(
            0,
            Ty::named(bool_),
            &[Ty::Named {
                name: string,
                nullable: true,
                args: ArgListId::EMPTY,
            }],
            &[],
            false,
        );
        let want = pool.function(0, Ty::Void, &[want_formal], &[], false);
        let got = pool.map_leaves(member, &mut |leaf| match leaf {
            Ty::Named { name, .. } if name == e => Ty::Named {
                name: string,
                nullable: true,
                args: ArgListId::EMPTY,
            },
            other => other,
        });
        assert_eq!(got, want);
    }

    #[test]
    fn an_untouched_type_keeps_its_identity_and_a_generic_shape_is_opaque() {
        let (mut pool, strings) = rig();
        let (t, int) = (strings.intern("T"), strings.intern("int"));
        let generic = pool.function(1, Ty::named(t), &[Ty::named(t)], &[], false);
        let record = pool.record(&[Ty::named(int), generic], &[], false);
        let mut hits = 0;
        let got = pool.map_leaves(record, &mut |leaf| {
            hits += 1;
            leaf
        });
        assert_eq!(got, record);
        assert_eq!(hits, 1, "only the record's plain `int` field is a leaf");
        let mut seen = Vec::new();
        pool.visit_named(record, &mut |named| seen.push(named));
        assert_eq!(seen, vec![Ty::named(int)]);
    }
}
