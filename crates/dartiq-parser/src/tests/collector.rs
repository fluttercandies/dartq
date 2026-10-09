//! Shared test scaffolding: scan helper, event collector, parser setup.

use dartiq_language::{FeatureSet, LanguageVersion};
use dartiq_scanner::{ScanConfig, TokenTable};
use dartiq_source::FileTable;

use crate::listener::{ExperimentalFlag, ParserListener};
use crate::message::Message;
use crate::parser::Parser;
use crate::listener::Token;
use crate::token_id::TokenSpan;

/// Scans `source` with the default (latest) configuration and asserts no
/// scanner diagnostics. Returns the leaked table + source so cursors can be
/// `'static` in test helpers.
pub(crate) fn scan_ok(source: &'static str) -> (&'static TokenTable, &'static str) {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartiq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let table: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    (table, text)
}

/// Latest-version feature set.
pub(crate) fn latest_features() -> crate::ParserFeatures {
    crate::ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        3, 15,
    )))
}

/// Flat event log entry: `("begin", "pattern")` style.
pub(crate) type Logged = (&'static str, &'static str);

/// Listener that records recoverable errors (code, start, end in UTF-16)
/// and a log of the structural events the tests observe.
#[derive(Default)]
pub(crate) struct Collector {
    pub errors: Vec<(String, u32, u32)>,
    pub log: Vec<Logged>,
}

impl Collector {
    pub(crate) fn error_codes(&self) -> Vec<String> {
        self.errors.iter().map(|(c, _, _)| c.clone()).collect()
    }

    /// Subsequence check over the log: every `(phase, name)` appears in
    /// order (gaps allowed).
    pub(crate) fn assert_sequence(&self, expected: &[Logged]) {
        let mut it = self.log.iter();
        for want in expected {
            assert!(
                it.any(|got| got == want),
                "expected {want:?} in order; log so far: {:?} (full: {:?})",
                self.log,
                self.errors
            );
        }
    }
}

impl ParserListener for Collector {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, end: TokenSpan) {
        self.errors.push((
            message.code.name.to_owned(),
            start.range.offset,
            end.range.offset + end.range.length,
        ));
    }

    fn handle_experiment_not_enabled(
        &mut self,
        _flag: ExperimentalFlag,
        begin: TokenSpan,
        _end: TokenSpan,
    ) {
        self.errors.push((
            "experiment_not_enabled".into(),
            begin.range.offset,
            begin.range.offset,
        ));
    }

    // —— representative structural events for sequence assertions ————
    fn begin_pattern(&mut self, _: TokenSpan) {
        self.log.push(("begin", "pattern"));
    }
    fn end_pattern(&mut self, _: TokenSpan) {
        self.log.push(("end", "pattern"));
    }
    fn begin_constant_pattern(&mut self, _: Option<TokenSpan>) {
        self.log.push(("begin", "constant_pattern"));
    }
    fn end_constant_pattern(&mut self, _: Option<TokenSpan>) {
        self.log.push(("end", "constant_pattern"));
    }
    fn handle_type(&mut self, _: TokenSpan, _: Option<TokenSpan>) {
        self.log.push(("handle", "type"));
    }
    fn handle_no_type(&mut self, _: TokenSpan) {
        self.log.push(("handle", "no_type"));
    }
    fn begin_type_arguments(&mut self, _: TokenSpan) {
        self.log.push(("begin", "type_arguments"));
    }
    fn end_type_arguments(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "type_arguments"));
    }
    fn begin_record_type(&mut self, _: TokenSpan) {
        self.log.push(("begin", "record_type"));
    }
    fn end_record_type(&mut self, _: TokenSpan, _: Option<TokenSpan>, _: usize, _: bool) {
        self.log.push(("end", "record_type"));
    }
    fn begin_function_type(&mut self, _: TokenSpan) {
        self.log.push(("begin", "function_type"));
    }
    fn end_function_type(&mut self, _: TokenSpan, _: Option<TokenSpan>) {
        self.log.push(("end", "function_type"));
    }
    fn handle_list_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "list_pattern"));
    }
    fn handle_map_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "map_pattern"));
    }
    fn handle_record_pattern(&mut self, _: TokenSpan, _: usize) {
        self.log.push(("handle", "record_pattern"));
    }
    fn handle_parenthesized_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "parenthesized_pattern"));
    }
    fn handle_object_pattern(&mut self, _: TokenSpan, _: Option<TokenSpan>, _: Option<TokenSpan>) {
        self.log.push(("handle", "object_pattern"));
    }
    fn handle_wildcard_pattern(&mut self, _: Option<TokenSpan>, _: TokenSpan) {
        self.log.push(("handle", "wildcard_pattern"));
    }
    fn handle_declared_variable_pattern(&mut self, _: Option<TokenSpan>, _: TokenSpan, _: bool) {
        self.log.push(("handle", "declared_variable_pattern"));
    }
    fn handle_assigned_variable_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "assigned_variable_pattern"));
    }
    fn handle_cast_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "cast_pattern"));
    }
    fn handle_null_assert_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "null_assert_pattern"));
    }
    fn handle_null_check_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "null_check_pattern"));
    }
    fn begin_binary_pattern(&mut self, _: TokenSpan) {
        self.log.push(("begin", "binary_pattern"));
    }
    fn end_binary_pattern(&mut self, _: TokenSpan) {
        self.log.push(("end", "binary_pattern"));
    }
    fn handle_relational_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "relational_pattern"));
    }
    fn handle_literal_int(&mut self, _: TokenSpan) {
        self.log.push(("handle", "literal_int"));
    }
    fn handle_literal_bool(&mut self, _: TokenSpan) {
        self.log.push(("handle", "literal_bool"));
    }

    // —— declarations (#18) ————————————————————————————————————————————
    fn begin_class_declaration(&mut self, _: Token, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Token) {
        self.log.push(("begin", "class_declaration"));
    }
    fn end_class_declaration(&mut self, _: Token, _: Token) {
        self.log.push(("end", "class_declaration"));
    }
    fn begin_named_mixin_application(&mut self, _: Token, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Token) {
        self.log.push(("begin", "named_mixin_application"));
    }
    fn end_named_mixin_application(
        &mut self,
        _: Token,
        _: Token,
        _: Token,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("end", "named_mixin_application"));
    }
    fn begin_mixin_declaration(&mut self, _: Token, _: Option<Token>, _: Option<Token>, _: Token, _: Token) {
        self.log.push(("begin", "mixin_declaration"));
    }
    fn end_mixin_declaration(&mut self, _: Token, _: Token) {
        self.log.push(("end", "mixin_declaration"));
    }
    fn begin_enum_declaration(&mut self, _: Token, _: Option<Token>, _: Token, _: Token) {
        self.log.push(("begin", "enum_declaration"));
    }
    fn end_enum_declaration(&mut self, _: Token, _: Token, _: Token, _: usize, _: Token) {
        self.log.push(("end", "enum_declaration"));
    }
    fn begin_extension_declaration(&mut self, _: Option<Token>, _: Token, _: Option<Token>) {
        self.log.push(("begin", "extension_declaration"));
    }
    fn end_extension_declaration(&mut self, _: Token, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "extension_declaration"));
    }
    fn begin_extension_type_declaration(&mut self, _: Option<Token>, _: Token, _: Token) {
        self.log.push(("begin", "extension_type_declaration"));
    }
    fn end_extension_type_declaration(&mut self, _: Token, _: Option<Token>, _: Token, _: Token, _: Token) {
        self.log.push(("end", "extension_type_declaration"));
    }
    fn begin_typedef(&mut self, _: Token) {
        self.log.push(("begin", "typedef"));
    }
    fn end_typedef(&mut self, _: Option<Token>, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "typedef"));
    }
    fn begin_top_level_member(&mut self, _: Token) {
        self.log.push(("begin", "top_level_member"));
    }
    fn begin_method(&mut self, _: crate::listener::DeclarationKind, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Token, _: Option<&str>) {
        self.log.push(("begin", "method"));
    }
    fn end_method(&mut self, _: crate::listener::DeclarationKind, _: Option<Token>, _: Token, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "method"));
    }
    fn begin_constructor(&mut self, _: crate::listener::DeclarationKind, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Token, _: Option<&str>) {
        self.log.push(("begin", "constructor"));
    }
    fn end_constructor(&mut self, _: crate::listener::DeclarationKind, _: Token, _: Option<Token>, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "constructor"));
    }
    fn begin_factory(&mut self, _: crate::listener::DeclarationKind, _: Token, _: Option<Token>, _: Option<Token>, _: Option<Token>) {
        self.log.push(("begin", "factory"));
    }
    fn end_factory(&mut self, _: crate::listener::DeclarationKind, _: Token, _: Token, _: Token) {
        self.log.push(("end", "factory"));
    }
    fn begin_fields(&mut self, _: crate::listener::DeclarationKind, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Option<Token>, _: Token) {
        self.log.push(("begin", "fields"));
    }
    fn end_fields(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: usize,
        _: Token,
        _: Token,
    ) {
        self.log.push(("end", "fields"));
    }
    fn begin_class_or_mixin_or_extension_body(&mut self, _: crate::listener::DeclarationKind, _: Token) {
        self.log.push(("begin", "body"));
    }
    fn end_class_or_mixin_or_extension_body(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: usize,
        _: Token,
        _: Token,
    ) {
        self.log.push(("end", "body"));
    }
    fn handle_no_class_body(&mut self, _: Token) {
        self.log.push(("handle", "no_class_body"));
    }
}

/// Builds a parser over `source` with a fresh [`Collector`].
pub(crate) fn setup(source: &'static str) -> Parser<'static, 'static, Collector> {
    let (table, text) = scan_ok(source);
    Parser::new(text, table, latest_features(), Collector::default())
}
