//! Shared fixture: a miniature Dart core hierarchy plus user classes.

use dartiq_types::{
    CoreDecls, NamedMember, TypeDeclId, TypeHierarchy, TypeId, TypeParamId, TypeSystem, TypeTable,
    UninternedFunction, UninternedRecord, UninternedType,
};

/// Declaration and type-parameter handles for the fixture.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct Decls {
    pub object: TypeDeclId,
    pub future: TypeDeclId,
    pub function: TypeDeclId,
    pub num: TypeDeclId,
    pub int: TypeDeclId,
    pub double: TypeDeclId,
    pub string: TypeDeclId,
    pub boolean: TypeDeclId,
    pub iterable: TypeDeclId,
    pub list: TypeDeclId,
    pub animal: TypeDeclId,
    pub cat: TypeDeclId,
    pub dog: TypeDeclId,
    pub iterable_t: TypeParamId,
    pub list_e: TypeParamId,
}

/// Table + validated hierarchy snapshot + typed shortcuts.
pub struct Fixture {
    pub table: TypeTable,
    pub hierarchy: TypeHierarchy,
    pub d: Decls,
}

#[allow(dead_code)]
pub fn fixture() -> Fixture {
    let d = Decls {
        object: TypeDeclId::from_raw(1),
        future: TypeDeclId::from_raw(2),
        function: TypeDeclId::from_raw(3),
        num: TypeDeclId::from_raw(4),
        int: TypeDeclId::from_raw(5),
        double: TypeDeclId::from_raw(6),
        string: TypeDeclId::from_raw(7),
        boolean: TypeDeclId::from_raw(8),
        iterable: TypeDeclId::from_raw(9),
        list: TypeDeclId::from_raw(10),
        animal: TypeDeclId::from_raw(11),
        cat: TypeDeclId::from_raw(12),
        dog: TypeDeclId::from_raw(13),
        iterable_t: TypeParamId::from_raw(1),
        list_e: TypeParamId::from_raw(2),
    };
    let table = TypeTable::new();

    let list_of_e = table.intern(
        UninternedType::TypeParameter {
            parameter: d.list_e,
        },
        false,
    );

    let mut b = TypeHierarchy::builder(CoreDecls {
        object: d.object,
        future: d.future,
        function: d.function,
        // No `Record` class in the shared fixture: record LUB stays on the
        // `UP(Object, T2)` fallback (the Record-class LUB test builds its own
        // snapshot with `record: Some(..)`).
        record: None,
    })
    .decl(d.object, Vec::new())
    .decl(d.future, vec![d.iterable_t])
    .decl(d.function, Vec::new())
    .decl(d.num, Vec::new())
    .decl(d.int, Vec::new())
    .decl(d.double, Vec::new())
    .decl(d.string, Vec::new())
    .decl(d.boolean, Vec::new())
    .decl(d.iterable, vec![d.iterable_t])
    .decl(d.list, vec![d.list_e])
    .decl(d.animal, Vec::new())
    .decl(d.cat, Vec::new())
    .decl(d.dog, Vec::new());

    let object_none = itf(&table, d.object, &[], false);

    b = b
        .supertype(d.num, object_none)
        .supertype(d.int, itf(&table, d.num, &[], false))
        .supertype(d.double, itf(&table, d.num, &[], false))
        .supertype(d.string, object_none)
        .supertype(d.boolean, object_none)
        .supertype(d.iterable, object_none)
        .supertype(d.list, itf(&table, d.iterable, &[list_of_e], false))
        .supertype(d.future, object_none)
        .supertype(d.function, object_none)
        .supertype(d.animal, object_none)
        .supertype(d.cat, itf(&table, d.animal, &[], false))
        .supertype(d.dog, itf(&table, d.animal, &[], false));

    let hierarchy = b.build(&table).expect("fixture hierarchy is valid");
    Fixture {
        table,
        hierarchy,
        d,
    }
}

#[allow(dead_code)]
impl Fixture {
    pub fn system(&self) -> TypeSystem<'_> {
        TypeSystem::new(&self.table, &self.hierarchy)
    }

    /// Interns an interface instantiation.
    pub fn itf(&self, decl: TypeDeclId, args: &[TypeId], nullable: bool) -> TypeId {
        itf(&self.table, decl, args, nullable)
    }

    pub fn unit(&self, ty: UninternedType, nullable: bool) -> TypeId {
        self.table.intern(ty, nullable)
    }

    pub fn int(&self) -> TypeId {
        self.itf(self.d.int, &[], false)
    }
    pub fn int_question(&self) -> TypeId {
        self.itf(self.d.int, &[], true)
    }
    pub fn num(&self) -> TypeId {
        self.itf(self.d.num, &[], false)
    }
    pub fn string(&self) -> TypeId {
        self.itf(self.d.string, &[], false)
    }
    pub fn string_question(&self) -> TypeId {
        self.itf(self.d.string, &[], true)
    }
    pub fn double(&self) -> TypeId {
        self.itf(self.d.double, &[], false)
    }
    pub fn animal_question(&self) -> TypeId {
        self.itf(self.d.animal, &[], true)
    }
    pub fn animal(&self) -> TypeId {
        self.itf(self.d.animal, &[], false)
    }
    pub fn cat(&self) -> TypeId {
        self.itf(self.d.cat, &[], false)
    }
    pub fn dog(&self) -> TypeId {
        self.itf(self.d.dog, &[], false)
    }
    pub fn cat_question(&self) -> TypeId {
        self.itf(self.d.cat, &[], true)
    }
    pub fn list_of(&self, arg: TypeId, nullable: bool) -> TypeId {
        self.itf(self.d.list, &[arg], nullable)
    }
    pub fn iterable_of(&self, arg: TypeId, nullable: bool) -> TypeId {
        self.itf(self.d.iterable, &[arg], nullable)
    }
    pub fn future_of(&self, arg: TypeId) -> TypeId {
        self.itf(self.d.future, &[arg], false)
    }
    pub fn future_or_of(&self, arg: TypeId, nullable: bool) -> TypeId {
        self.table
            .intern(UninternedType::FutureOr { base: arg }, nullable)
    }
    pub fn type_param(&self, parameter: TypeParamId) -> TypeId {
        self.table
            .intern(UninternedType::TypeParameter { parameter }, false)
    }

    /// `(positional...) -> ret` with everything positional required.
    pub fn fn_type(&self, positional: &[TypeId], ret: TypeId) -> TypeId {
        self.fn_sig(UninternedFunction {
            return_type: ret,
            type_parameters: Vec::new(),
            positional: positional.to_vec(),
            named: Vec::new(),
            required_positional: u32::try_from(positional.len()).expect("small fixture"),
        })
    }

    pub fn fn_sig(&self, sig: UninternedFunction) -> TypeId {
        self.table
            .intern_function(sig, false)
            .expect("valid fixture signature")
    }

    pub fn record(&self, positional: &[TypeId], named: &[NamedMember]) -> TypeId {
        self.table
            .intern_record(
                UninternedRecord {
                    positional: positional.to_vec(),
                    named: named.to_vec(),
                },
                false,
            )
            .expect("valid fixture record")
    }

    pub fn never(&self) -> TypeId {
        self.table.intern(UninternedType::Never, false)
    }
    pub fn dynamic(&self) -> TypeId {
        self.table.intern(UninternedType::Dynamic, false)
    }
    pub fn void_(&self) -> TypeId {
        self.table.intern(UninternedType::Void, false)
    }
    pub fn null(&self) -> TypeId {
        self.table.intern(UninternedType::Null, false)
    }
    pub fn invalid(&self) -> TypeId {
        self.table.intern(UninternedType::Invalid, false)
    }
    pub fn object_question(&self) -> TypeId {
        self.itf(self.d.object, &[], true)
    }
}

pub fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId], nullable: bool) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: args.to_vec(),
        },
        nullable,
    )
}

#[allow(dead_code)]
pub fn named(name: &str, ty: TypeId, required: bool) -> NamedMember {
    NamedMember {
        name: name.to_string(),
        ty,
        required,
    }
}
