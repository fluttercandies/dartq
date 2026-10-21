//! The `is`-promotion pins over bounded generic hierarchies (the drift
//! corpus shapes `column is GeneratedColumn` over an `Expression` and
//! `table is VirtualTableInfo` over a `TableInfo`): a raw tested spelling
//! instantiates to its bounds on both sides of the strict promotion gate,
//! so `GeneratedColumn<Base> <: Expression<Base>` promotes through the
//! `Column<T>` hop and the promoted argument satisfies the narrower sink.
//! The rig loads no SDK, so the bound is a declared `Base` where the
//! corpus writes `Object`.
//!
//! Oracle provenance (`dart analyze` 3.13.1): both shapes are silent.

use super::test_rig::{assert_silent, report_ops};

#[test]
fn raw_is_test_promotes_through_bounded_class_chain() {
    let report = report_ops(
        "class Base {}\n\
         abstract class Expression<D extends Base> {}\n\
         abstract class Column<T extends Base> extends Expression<T> {}\n\
         class GeneratedColumn<T extends Base> extends Column<T> {}\n\
         void g(GeneratedColumn c) {}\n\
         void f(Expression column) {\n\
           if (column is GeneratedColumn) {\n\
             g(column);\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn raw_is_test_promotes_through_bounded_mixin_chain() {
    let report = report_ops(
        "class Table {}\n\
         mixin TableInfo<TableDsl extends Table, D> on Table {}\n\
         mixin VirtualTableInfo<TableDsl extends Table, D> on TableInfo<TableDsl, D> {}\n\
         void v(VirtualTableInfo t) {}\n\
         void c(TableInfo table) {\n\
           if (table is VirtualTableInfo) {\n\
             v(table);\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}
