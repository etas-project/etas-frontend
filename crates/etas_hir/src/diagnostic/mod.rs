pub mod codes;

pub use codes::*;

use etas_core::{
    Diagnostic, DiagnosticCode, DiagnosticLabel, DiagnosticPhase, DiagnosticSpan, LabelStyle,
    NameDiagnosticCode, Severity, Span,
};

#[derive(Clone, Debug, Default)]
pub struct HirDiagnostics {
    diagnostics: Vec<Diagnostic>,
}

impl HirDiagnostics {
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn finish(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn duplicate_symbol(&mut self, name: &str, span: Span, previous: Span) {
        self.push(
            name_diagnostic(
                NameDiagnosticCode::DuplicateSymbol,
                span,
                format!("duplicate symbol `{name}` in this scope"),
            )
            .with_secondary(previous, "previous definition is here"),
        );
    }

    pub fn unresolved_name(&mut self, name: &str, span: Span) {
        self.push(name_diagnostic(
            NameDiagnosticCode::UnresolvedName,
            span,
            format!("unresolved name `{name}`"),
        ));
    }

    pub fn ambiguous_name(&mut self, name: &str, span: Span) {
        self.push(name_diagnostic(
            NameDiagnosticCode::AmbiguousName,
            span,
            format!("ambiguous name `{name}`"),
        ));
    }

    pub fn import_alias_conflict(&mut self, alias: &str, span: Span, previous: Span) {
        self.push(
            name_diagnostic(
                NameDiagnosticCode::ImportAliasConflict,
                span,
                format!("import alias `{alias}` conflicts with an existing symbol"),
            )
            .with_secondary(previous, "conflicting symbol is defined here"),
        );
    }

    pub fn invalid_import_path(&mut self, path: &str, span: Span) {
        self.push(name_diagnostic(
            NameDiagnosticCode::InvalidImportPath,
            span,
            format!("invalid import path `{path}`"),
        ));
    }

    pub fn unsupported_qualified_path(&mut self, path: &str, span: Span) {
        let mut diagnostic = name_diagnostic(
            NameDiagnosticCode::UnsupportedQualifiedPath,
            span,
            format!("qualified path `{path}` cannot be fully resolved yet"),
        );
        diagnostic.help = Some(
            "only exact local symbols and import aliases are resolved in this HIR slice".into(),
        );
        self.push(diagnostic);
    }

    pub fn invalid_impl_target(&mut self, span: Span) {
        self.push(name_diagnostic(
            NameDiagnosticCode::InvalidImplTarget,
            span,
            "invalid impl target path",
        ));
    }

    pub fn unresolved_effect_action(&mut self, action: &str, span: Span) {
        self.push(name_diagnostic(
            NameDiagnosticCode::UnresolvedEffectAction,
            span,
            format!("unresolved effect action `{action}`"),
        ));
    }
}

fn name_diagnostic(code: NameDiagnosticCode, span: Span, message: impl Into<String>) -> Diagnostic {
    let message = message.into();
    Diagnostic {
        code: DiagnosticCode::Name(code),
        phase: DiagnosticPhase::NameResolution,
        severity: Severity::Error,
        message: message.clone(),
        primary: DiagnosticSpan {
            span,
            label: Some(message.clone()),
        },
        labels: vec![DiagnosticLabel {
            span,
            style: LabelStyle::Primary,
            message,
        }],
        notes: Vec::new(),
        help: None,
        suggestions: Vec::new(),
    }
}
