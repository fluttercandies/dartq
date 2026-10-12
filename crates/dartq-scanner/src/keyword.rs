//! Full Dart keyword set and classification (aligned with the official
//! `token.dart` `Keyword` table, Dart 3.15).
//!
//! The scanner emits [`TokenKind::Keyword`] for pseudo keywords and built-ins
//! alike; context-based disambiguation is the parser's job. `augment` is the
//! one lexically gated keyword: with [`crate::ScanConfig::enable_augmentations`]
//! disabled it lexes as an identifier, mirroring the official scanner's
//! `tokenizeKeywordOrIdentifier` (`abstract_scanner.dart:2106` —
//! `if (!_enableAugmentations && keyword == Keyword.AUGMENT) return
//! tokenizeIdentifier(...)`) and the SDK's own scanner test pinning
//! identifier-when-disabled (`front_end` scanner_test.dart:480-489). With the
//! feature enabled it is `Keyword.AUGMENT` (token.dart:139-145,
//! `KeywordStyle.builtIn`).

/// Keyword classification (official `KeywordStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeywordStyle {
    /// Reserved words (cannot be used as identifiers).
    Reserved,
    /// Built-in identifier (usable as an identifier in most positions).
    BuiltIn,
    /// Pseudo/contextual keywords (identifier semantics decided by context).
    Pseudo,
}

/// Dart keywords (73, the official full set; includes the capitalized `Function`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Abstract,
    As,
    Assert,
    Async,
    Augment,
    Await,
    Base,
    Break,
    Case,
    Catch,
    Class,
    Const,
    Continue,
    Covariant,
    Default,
    Deferred,
    Do,
    Dynamic,
    Else,
    Enum,
    Export,
    Extends,
    Extension,
    External,
    Factory,
    False,
    Final,
    Finally,
    For,
    Function,
    Get,
    Hide,
    If,
    Implements,
    Import,
    In,
    Inout,
    Interface,
    Is,
    Late,
    Library,
    Mixin,
    Native,
    New,
    Null,
    Of,
    On,
    Operator,
    Out,
    Part,
    Patch,
    Required,
    Rethrow,
    Return,
    Sealed,
    Set,
    Show,
    Source,
    Static,
    Super,
    Switch,
    Sync,
    This,
    Throw,
    True,
    Try,
    Typedef,
    Var,
    Void,
    When,
    While,
    With,
    Yield,
}

impl Keyword {
    /// Keyword lexeme (one-to-one with variants).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Abstract => "abstract",
            Keyword::As => "as",
            Keyword::Assert => "assert",
            Keyword::Async => "async",
            Keyword::Augment => "augment",
            Keyword::Await => "await",
            Keyword::Base => "base",
            Keyword::Break => "break",
            Keyword::Case => "case",
            Keyword::Catch => "catch",
            Keyword::Class => "class",
            Keyword::Const => "const",
            Keyword::Continue => "continue",
            Keyword::Covariant => "covariant",
            Keyword::Default => "default",
            Keyword::Deferred => "deferred",
            Keyword::Do => "do",
            Keyword::Dynamic => "dynamic",
            Keyword::Else => "else",
            Keyword::Enum => "enum",
            Keyword::Export => "export",
            Keyword::Extends => "extends",
            Keyword::Extension => "extension",
            Keyword::External => "external",
            Keyword::Factory => "factory",
            Keyword::False => "false",
            Keyword::Final => "final",
            Keyword::Finally => "finally",
            Keyword::For => "for",
            Keyword::Function => "Function",
            Keyword::Get => "get",
            Keyword::Hide => "hide",
            Keyword::If => "if",
            Keyword::Implements => "implements",
            Keyword::Import => "import",
            Keyword::In => "in",
            Keyword::Inout => "inout",
            Keyword::Interface => "interface",
            Keyword::Is => "is",
            Keyword::Late => "late",
            Keyword::Library => "library",
            Keyword::Mixin => "mixin",
            Keyword::Native => "native",
            Keyword::New => "new",
            Keyword::Null => "null",
            Keyword::Of => "of",
            Keyword::On => "on",
            Keyword::Operator => "operator",
            Keyword::Out => "out",
            Keyword::Part => "part",
            Keyword::Patch => "patch",
            Keyword::Required => "required",
            Keyword::Rethrow => "rethrow",
            Keyword::Return => "return",
            Keyword::Sealed => "sealed",
            Keyword::Set => "set",
            Keyword::Show => "show",
            Keyword::Source => "source",
            Keyword::Static => "static",
            Keyword::Super => "super",
            Keyword::Switch => "switch",
            Keyword::Sync => "sync",
            Keyword::This => "this",
            Keyword::Throw => "throw",
            Keyword::True => "true",
            Keyword::Try => "try",
            Keyword::Typedef => "typedef",
            Keyword::Var => "var",
            Keyword::Void => "void",
            Keyword::When => "when",
            Keyword::While => "while",
            Keyword::With => "with",
            Keyword::Yield => "yield",
        }
    }

    /// Official classification (`KeywordStyle`).
    #[must_use]
    pub fn style(self) -> KeywordStyle {
        match self {
            // reserved
            Keyword::Assert
            | Keyword::Break
            | Keyword::Case
            | Keyword::Catch
            | Keyword::Class
            | Keyword::Const
            | Keyword::Continue
            | Keyword::Default
            | Keyword::Do
            | Keyword::Else
            | Keyword::Enum
            | Keyword::Extends
            | Keyword::False
            | Keyword::Final
            | Keyword::Finally
            | Keyword::For
            | Keyword::If
            | Keyword::In
            | Keyword::Is
            | Keyword::New
            | Keyword::Null
            | Keyword::Rethrow
            | Keyword::Return
            | Keyword::Super
            | Keyword::Switch
            | Keyword::This
            | Keyword::Throw
            | Keyword::True
            | Keyword::Try
            | Keyword::Var
            | Keyword::Void
            | Keyword::While
            | Keyword::With => KeywordStyle::Reserved,
            // built-in
            Keyword::Abstract
            | Keyword::As
            | Keyword::Augment
            | Keyword::Covariant
            | Keyword::Deferred
            | Keyword::Dynamic
            | Keyword::Export
            | Keyword::Extension
            | Keyword::External
            | Keyword::Factory
            | Keyword::Function
            | Keyword::Get
            | Keyword::Implements
            | Keyword::Import
            | Keyword::Interface
            | Keyword::Late
            | Keyword::Library
            | Keyword::Mixin
            | Keyword::Operator
            | Keyword::Part
            | Keyword::Required
            | Keyword::Set
            | Keyword::Static
            | Keyword::Typedef => KeywordStyle::BuiltIn,
            // pseudo
            Keyword::Async
            | Keyword::Await
            | Keyword::Base
            | Keyword::Hide
            | Keyword::Inout
            | Keyword::Native
            | Keyword::Of
            | Keyword::On
            | Keyword::Out
            | Keyword::Patch
            | Keyword::Sealed
            | Keyword::Show
            | Keyword::Source
            | Keyword::Sync
            | Keyword::When
            | Keyword::Yield => KeywordStyle::Pseudo,
        }
    }
}

/// Lexeme to keyword (exact match; the caller guarantees the lexeme comes from
/// a full identifier scan).
#[must_use]
pub fn keyword_from_str(s: &str) -> Option<Keyword> {
    // rustc compiles the 73-arm str match into a length+prefix decision tree;
    // no DFA table needed.
    Some(match s {
        "abstract" => Keyword::Abstract,
        "as" => Keyword::As,
        "assert" => Keyword::Assert,
        "async" => Keyword::Async,
        "augment" => Keyword::Augment,
        "await" => Keyword::Await,
        "base" => Keyword::Base,
        "break" => Keyword::Break,
        "case" => Keyword::Case,
        "catch" => Keyword::Catch,
        "class" => Keyword::Class,
        "const" => Keyword::Const,
        "continue" => Keyword::Continue,
        "covariant" => Keyword::Covariant,
        "default" => Keyword::Default,
        "deferred" => Keyword::Deferred,
        "do" => Keyword::Do,
        "dynamic" => Keyword::Dynamic,
        "else" => Keyword::Else,
        "enum" => Keyword::Enum,
        "export" => Keyword::Export,
        "extends" => Keyword::Extends,
        "extension" => Keyword::Extension,
        "external" => Keyword::External,
        "factory" => Keyword::Factory,
        "false" => Keyword::False,
        "final" => Keyword::Final,
        "finally" => Keyword::Finally,
        "for" => Keyword::For,
        "Function" => Keyword::Function,
        "get" => Keyword::Get,
        "hide" => Keyword::Hide,
        "if" => Keyword::If,
        "implements" => Keyword::Implements,
        "import" => Keyword::Import,
        "in" => Keyword::In,
        "inout" => Keyword::Inout,
        "interface" => Keyword::Interface,
        "is" => Keyword::Is,
        "late" => Keyword::Late,
        "library" => Keyword::Library,
        "mixin" => Keyword::Mixin,
        "native" => Keyword::Native,
        "new" => Keyword::New,
        "null" => Keyword::Null,
        "of" => Keyword::Of,
        "on" => Keyword::On,
        "operator" => Keyword::Operator,
        "out" => Keyword::Out,
        "part" => Keyword::Part,
        "patch" => Keyword::Patch,
        "required" => Keyword::Required,
        "rethrow" => Keyword::Rethrow,
        "return" => Keyword::Return,
        "sealed" => Keyword::Sealed,
        "set" => Keyword::Set,
        "show" => Keyword::Show,
        "source" => Keyword::Source,
        "static" => Keyword::Static,
        "super" => Keyword::Super,
        "switch" => Keyword::Switch,
        "sync" => Keyword::Sync,
        "this" => Keyword::This,
        "throw" => Keyword::Throw,
        "true" => Keyword::True,
        "try" => Keyword::Try,
        "typedef" => Keyword::Typedef,
        "var" => Keyword::Var,
        "void" => Keyword::Void,
        "when" => Keyword::When,
        "while" => Keyword::While,
        "with" => Keyword::With,
        "yield" => Keyword::Yield,
        _ => return None,
    })
}
