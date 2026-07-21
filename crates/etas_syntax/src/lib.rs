pub mod ast;
mod cursor;
pub mod dump;
mod grammar;
mod lexer;
pub mod parse;
mod parser;
pub mod token;
pub mod trivia;

pub use dump::{AstDump, AstDumpWriter, DumpOptions, dump_ast, dump_parse};
pub use etas_core::{
    AnalysisDiagnosticCode, Applicability, Diagnostic, DiagnosticCode, DiagnosticLabel,
    DiagnosticPhase, DiagnosticSpan, EffectDiagnosticCode, LabelStyle, LineCol, LineIndex,
    NameDiagnosticCode, Severity, SourceFile, SourceId, Span, Suggestion, SyntaxDiagnosticCode,
    TextEdit, TextRange, TextSize, TypeDiagnosticCode,
};
pub use lexer::lex;
pub use parse::Parse;
pub use parser::parse_program;
pub use token::{CommentKind, Keyword, Punct, Token, TokenKind, TokenStream};
pub use trivia::{Trivia, TriviaKind};
