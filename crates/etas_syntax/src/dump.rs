use crate::{
    Diagnostic, LabelStyle, Parse, Severity, Span, TextRange,
    ast::{
        self, Arg, BinaryOp, DeclarationConformance, DeclarationConformanceTarget, ElseBranch,
        Expr, FieldInit, ImplItem, LambdaBody, LambdaParams, Literal, MatchArmBody,
        MemoryAccessKind, Pattern, Stmt, TypeExpr, UnaryOp,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpOptions {
    pub include_spans: bool,
    pub include_tokens: bool,
    pub include_diagnostics: bool,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            include_spans: true,
            include_tokens: true,
            include_diagnostics: false,
        }
    }
}

pub trait AstDump {
    fn dump(&self, f: &mut AstDumpWriter);
}

#[derive(Clone, Debug)]
pub struct AstDumpWriter {
    output: String,
    indent: usize,
    options: DumpOptions,
}

impl AstDumpWriter {
    pub fn new(options: DumpOptions) -> Self {
        Self {
            output: String::new(),
            indent: 0,
            options,
        }
    }

    pub fn options(&self) -> DumpOptions {
        self.options
    }

    pub fn finish(self) -> String {
        self.output
    }

    pub fn line(&mut self, kind: &str, span: Span, fields: Vec<String>) {
        self.indent();
        self.output.push_str(kind);
        for field in fields {
            self.output.push(' ');
            self.output.push_str(&field);
        }
        if self.options.include_spans {
            self.output.push(' ');
            self.output.push_str(&format_span(span));
        }
        self.output.push('\n');
    }

    pub fn node(
        &mut self,
        kind: &str,
        span: Span,
        fields: Vec<String>,
        body: impl FnOnce(&mut Self),
    ) {
        self.line(kind, span, fields);
        self.indent += 1;
        body(self);
        self.indent -= 1;
    }

    fn diagnostics(&mut self, diagnostics: &[Diagnostic]) {
        if diagnostics.is_empty() {
            return;
        }

        self.indent();
        self.output.push_str("Diagnostics\n");
        self.indent += 1;
        for diagnostic in diagnostics {
            let mut fields = vec![
                format!("code={:?}", diagnostic.code),
                format!("phase={:?}", diagnostic.phase),
                format!("severity={}", severity_text(diagnostic.severity)),
            ];
            if self.options.include_tokens {
                fields.push(format!("message={:?}", diagnostic.message));
            }
            self.line("Diagnostic", diagnostic.primary.span, fields);

            self.indent += 1;
            for label in &diagnostic.labels {
                let mut fields = vec![format!("style={}", label_style_text(label.style))];
                if self.options.include_tokens {
                    fields.push(format!("message={:?}", label.message));
                }
                self.line("Label", label.span, fields);
            }
            for note in &diagnostic.notes {
                self.indent();
                self.output.push_str("Note");
                if self.options.include_tokens {
                    self.output.push_str(&format!(" text={note:?}"));
                }
                self.output.push('\n');
            }
            if let Some(help) = &diagnostic.help {
                self.indent();
                self.output.push_str("Help");
                if self.options.include_tokens {
                    self.output.push_str(&format!(" text={help:?}"));
                }
                self.output.push('\n');
            }
            for suggestion in &diagnostic.suggestions {
                self.indent();
                self.output.push_str("Suggestion");
                if self.options.include_tokens {
                    self.output
                        .push_str(&format!(" title={:?}", suggestion.title));
                }
                self.output
                    .push_str(&format!(" applicability={:?}\n", suggestion.applicability));
                self.indent += 1;
                for edit in &suggestion.edits {
                    self.indent();
                    self.output.push_str("Edit");
                    if self.options.include_spans {
                        self.output.push(' ');
                        self.output.push_str(&format_range(edit.range));
                    }
                    if self.options.include_tokens {
                        self.output
                            .push_str(&format!(" replacement={:?}", edit.replacement));
                    }
                    self.output.push('\n');
                }
                self.indent -= 1;
            }
            self.indent -= 1;
        }
        self.indent -= 1;
    }

    fn token_field(&self, name: &str, value: &str) -> Option<String> {
        self.options
            .include_tokens
            .then(|| format!("{name}={}", dump_atom(value)))
    }

    fn indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
    }
}

pub fn dump_ast(program: &ast::Program, options: DumpOptions) -> String {
    let mut writer = AstDumpWriter::new(options);
    program.dump(&mut writer);
    writer.finish()
}

pub fn dump_parse<T: AstDump>(parse: &Parse<T>, options: DumpOptions) -> String {
    let mut writer = AstDumpWriter::new(options);
    parse.value.dump(&mut writer);
    if options.include_diagnostics {
        writer.diagnostics(&parse.diagnostics);
    }
    writer.finish()
}

impl AstDump for ast::Program {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("Program", self.span, Vec::new(), |f| {
            if let Some(module) = &self.module {
                module.dump(f);
            }
            for import in &self.imports {
                import.dump(f);
            }
            for item in &self.items {
                item.dump(f);
            }
        });
    }
}

impl AstDump for ast::ModuleDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.line(
            "ModuleDecl",
            self.span,
            path_field(f, "path", &self.path).into_iter().collect(),
        );
    }
}

impl AstDump for ast::ImportDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        if f.options.include_tokens {
            fields.push(format!(
                "visibility={}",
                match self.visibility {
                    ast::Visibility::Private => "private",
                    ast::Visibility::Public => "public",
                }
            ));
        }
        f.node("ImportDecl", self.span, fields, |f| self.tree.dump(f));
    }
}

impl AstDump for ast::ImportTree {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Single { path, alias, span } => {
                let mut fields = Vec::new();
                push_path_field(f, &mut fields, "path", path);
                push_name_field(f, &mut fields, "alias", alias.as_ref());
                f.line("ImportSingle", *span, fields);
            }
            Self::Group {
                prefix,
                items,
                span,
            } => {
                let mut fields = Vec::new();
                push_path_field(f, &mut fields, "prefix", prefix);
                f.node("ImportGroup", *span, fields, |f| {
                    for item in items {
                        item.dump(f);
                    }
                });
            }
            Self::Wildcard {
                prefix,
                star_span,
                span,
            } => {
                let mut fields = Vec::new();
                push_path_field(f, &mut fields, "prefix", prefix);
                if f.options.include_spans {
                    fields.push(format!("star_span={}", format_span(*star_span)));
                }
                f.line("ImportWildcard", *span, fields);
            }
            Self::Error { span } => f.line("ImportError", *span, Vec::new()),
        }
    }
}

impl AstDump for ast::ImportItem {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_label_fields(f, "name", &self.name);
        push_name_field(f, &mut fields, "alias", self.alias.as_ref());
        f.line("ImportItem", self.span, fields);
    }
}

impl AstDump for ast::AnnotatedItem {
    fn dump(&self, f: &mut AstDumpWriter) {
        if self.annotations.is_empty() {
            self.item.dump(f);
            return;
        }
        f.node("AnnotatedItem", self.span, Vec::new(), |f| {
            for annotation in &self.annotations {
                annotation.dump(f);
            }
            self.item.dump(f);
        });
    }
}

impl AstDump for ast::Annotation {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        push_path_field(f, &mut fields, "path", &self.path);
        f.node("Annotation", self.span, fields, |f| {
            for arg in &self.args {
                arg.dump(f);
            }
        });
    }
}

impl AstDump for ast::AnnotationArg {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Positional(expr) => {
                f.node("AnnotationPositionalArg", expr.span(), Vec::new(), |f| {
                    expr.dump(f)
                });
            }
            Self::Named { name, value, span } => {
                f.node("AnnotationNamedArg", *span, name_fields(f, name), |f| {
                    value.dump(f)
                });
            }
        }
    }
}

impl AstDump for ast::Item {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Alias(item) => item.dump(f),
            Self::Type(item) => item.dump(f),
            Self::Enum(item) => item.dump(f),
            Self::Spec(item) => item.dump(f),
            Self::Impl(item) => item.dump(f),
            Self::Effect(item) => item.dump(f),
            Self::TopLevelLet(item) => item.dump(f),
            Self::Tool(item) => item.dump(f),
            Self::Agent(item) => item.dump(f),
            Self::Protocol(item) => item.dump(f),
            Self::Flow(item) => item.dump(f),
            Self::Error(item) => f.line("ErrorItem", item.span, Vec::new()),
        }
    }
}

impl AstDump for ast::TypeAliasDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("TypeAliasDecl", self.span, fields, |f| self.target.dump(f));
    }
}

impl AstDump for ast::TypeDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("TypeDecl", self.span, fields, |f| match &self.body {
            ast::TypeDeclBody::Bodyless => f.line("Bodyless", self.span, Vec::new()),
            ast::TypeDeclBody::Representation(ty) => ty.dump(f),
        });
    }
}

impl AstDump for ast::EnumDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("EnumDecl", self.span, fields, |f| {
            for variant in &self.variants {
                variant.dump(f);
            }
        });
    }
}

impl AstDump for ast::EnumVariant {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("EnumVariant", self.span, name_fields(f, &self.name), |f| {
            for field in &self.fields {
                field.dump(f);
            }
        });
    }
}

impl AstDump for ast::SpecDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        fields.push(format!("kind={}", spec_kind_text(self.kind)));
        push_type_params(f, &mut fields, &self.type_params);
        push_spec_bounds(f, &mut fields, &self.bounds);
        f.node("SpecDecl", self.span, fields, |f| {
            if let Some(callable) = &self.callable {
                callable.dump(f);
            }
            if let Some(trace) = &self.trace {
                trace.dump(f);
            }
            for item in &self.items {
                item.dump(f);
            }
        });
    }
}

impl AstDump for ast::SpecExpr {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Atom(pattern) => {
                f.node("SpecAtom", pattern.span, Vec::new(), |f| pattern.dump(f));
            }
            Self::Allow { pattern, span } => {
                f.node("TraceAllow", *span, Vec::new(), |f| pattern.dump(f));
            }
            Self::Deny { pattern, span } => {
                f.node("TraceDeny", *span, Vec::new(), |f| pattern.dump(f));
            }
            Self::And { lhs, rhs, span } => {
                f.node("TraceAnd", *span, Vec::new(), |f| {
                    lhs.dump(f);
                    rhs.dump(f);
                });
            }
            Self::Or { lhs, rhs, span } => {
                f.node("TraceOr", *span, Vec::new(), |f| {
                    lhs.dump(f);
                    rhs.dump(f);
                });
            }
            Self::Before {
                before,
                after,
                span,
            } => {
                f.node("TraceBefore", *span, Vec::new(), |f| {
                    before.dump(f);
                    after.dump(f);
                });
            }
            Self::After {
                after,
                before,
                span,
            } => {
                f.node("TraceAfter", *span, Vec::new(), |f| {
                    after.dump(f);
                    before.dump(f);
                });
            }
        }
    }
}

impl AstDump for ast::SpecCallableSignature {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("SpecCallableSignature", self.span, Vec::new(), |f| {
            self.input.dump(f);
            self.output.dump(f);
            if let Some(effects) = &self.effects {
                effects.dump(f);
            }
        });
    }
}

impl AstDump for ast::SpecItem {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::FlowSignature(signature) => signature.dump(f),
            Self::Error(span) => f.line("ErrorSpecItem", *span, Vec::new()),
        }
    }
}

impl AstDump for ast::FlowSignature {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("FlowSignature", self.span, fields, |f| {
            for param in &self.params {
                param.dump(f);
            }
            if let Some(ty) = &self.return_type {
                f.node("ReturnType", ty.span(), Vec::new(), |f| ty.dump(f));
            }
            if let Some(effects) = &self.declared_effects {
                f.node("EffectSuffix", effects.span, Vec::new(), |f| {
                    effects.dump(f)
                });
            }
        });
    }
}

impl AstDump for ast::ImplDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        match &self.target {
            ast::ImplTarget::Inherent {
                target, type_args, ..
            } => {
                push_path_field(f, &mut fields, "target", target);
                f.node("ImplDecl", self.span, fields, |f| {
                    for arg in type_args {
                        f.node("TypeArg", arg.span(), Vec::new(), |f| arg.dump(f));
                    }
                    for item in &self.items {
                        item.dump(f);
                    }
                });
            }
            ast::ImplTarget::SpecSatisfaction {
                specs, self_type, ..
            } => {
                f.node("SpecImplDecl", self.span, fields, |f| {
                    for spec_ref in specs {
                        let mut spec_fields = Vec::new();
                        push_path_field(f, &mut spec_fields, "spec", &spec_ref.spec_path);
                        f.node("ImplSpecRef", spec_ref.span, spec_fields, |f| {
                            for arg in &spec_ref.spec_args {
                                f.node("SpecTypeArg", arg.span(), Vec::new(), |f| arg.dump(f));
                            }
                        });
                    }
                    f.node("ForType", self_type.span(), Vec::new(), |f| {
                        self_type.dump(f)
                    });
                    for item in &self.items {
                        item.dump(f);
                    }
                });
            }
            ast::ImplTarget::Error { .. } => {
                f.node("ImplDeclError", self.span, fields, |f| {
                    for item in &self.items {
                        item.dump(f);
                    }
                });
            }
        }
    }
}

impl AstDump for ImplItem {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Flow(flow) => flow.dump(f),
            Self::Action(action) => action.dump(f),
            Self::Error(span) => f.line("ErrorImplItem", *span, Vec::new()),
        }
    }
}

impl AstDump for ast::EffectDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("EffectDecl", self.span, fields, |f| {
            if let Some(effect) = &self.extends {
                f.node("Extends", effect.span, Vec::new(), |f| effect.dump(f));
            }
            match &self.body {
                ast::EffectBody::Empty { span } => f.line("EmptyEffectBody", *span, Vec::new()),
                ast::EffectBody::Block { actions, span } => {
                    f.node("EffectBody", *span, Vec::new(), |f| {
                        for action in actions {
                            action.dump(f);
                        }
                    });
                }
            }
        });
    }
}

impl AstDump for ast::EffectActionDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        if !self.selector_params.is_empty() {
            fields.push(format!(
                "selectors={}",
                self.selector_params
                    .iter()
                    .map(|param| match param {
                        ast::ActionSelectorParam::Type(param) => param.name.text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        f.node("EffectActionDecl", self.span, fields, |f| {
            for param in &self.params {
                param.dump(f);
            }
            f.node("ReturnType", self.return_type.span(), Vec::new(), |f| {
                self.return_type.dump(f);
            });
        });
    }
}

impl AstDump for ast::FlowDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("FlowDecl", self.span, fields, |f| {
            for conformance in &self.conformances {
                conformance.dump(f);
            }
            for param in &self.params {
                param.dump(f);
            }
            if let Some(ty) = &self.return_type {
                f.node("ReturnType", ty.span(), Vec::new(), |f| ty.dump(f));
            }
            if let Some(effects) = &self.declared_effects {
                f.node("EffectSuffix", effects.span, Vec::new(), |f| {
                    effects.dump(f)
                });
            }
            match &self.body {
                ast::FlowBody::Block(block) => block.dump(f),
                ast::FlowBody::Expr { expr, span } => {
                    f.node("ExprBody", *span, Vec::new(), |f| expr.dump(f));
                }
            }
            if let Some(handler) = &self.trailing_handler {
                f.node("TrailingHandler", handler.span(), Vec::new(), |f| {
                    handler.dump(f);
                });
            }
        });
    }
}

impl AstDump for DeclarationConformance {
    fn dump(&self, f: &mut AstDumpWriter) {
        match &self.target {
            DeclarationConformanceTarget::Path(spec_ref) => {
                let mut fields = Vec::new();
                push_path_field(f, &mut fields, "path", &spec_ref.spec_path);
                f.node("DeclarationConformance", self.span, fields, |f| {
                    for arg in &spec_ref.spec_args {
                        arg.dump(f);
                    }
                });
            }
            DeclarationConformanceTarget::InlineTraceSpec(expr) => {
                f.node("InlineTraceSpecConformance", self.span, Vec::new(), |f| {
                    expr.dump(f);
                });
            }
            DeclarationConformanceTarget::Error(span) => {
                f.line("ErrorDeclarationConformance", *span, Vec::new())
            }
        }
    }
}

impl AstDump for ast::ToolDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        push_path_field(f, &mut fields, "path", &self.path);
        push_type_params(f, &mut fields, &self.type_params);
        f.node("ToolDecl", self.span, fields, |f| {
            for param in &self.params {
                param.dump(f);
            }
            f.node("ReturnType", self.return_type.span(), Vec::new(), |f| {
                self.return_type.dump(f);
            });
            if let Some(effects) = &self.effects {
                f.node("EffectSuffix", effects.span, Vec::new(), |f| {
                    effects.dump(f)
                });
            }
            for conformance in &self.conformances {
                conformance.dump(f);
            }
            self.body.dump(f);
        });
    }
}

impl AstDump for ast::ToolBody {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Source(body) => f.node("ToolSourceBody", body.span(), Vec::new(), |f| {
                body.dump(f);
            }),
            Self::Decl { semicolon_span } => f.line("ToolDeclBody", *semicolon_span, Vec::new()),
            Self::Error(span) => f.line("ErrorToolBody", *span, Vec::new()),
        }
    }
}

impl AstDump for ast::AgentDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("AgentDecl", self.span, name_fields(f, &self.name), |f| {
            for param in &self.params {
                param.dump(f);
            }
            if let Some(output_type) = &self.output_type {
                f.node("OutputType", output_type.span(), Vec::new(), |f| {
                    output_type.dump(f);
                });
            }
            if let Some(effects) = &self.effects {
                f.node("EffectSuffix", effects.span, Vec::new(), |f| {
                    effects.dump(f)
                });
            }
            for conformance in &self.conformances {
                conformance.dump(f);
            }
            self.body.dump(f);
        });
    }
}

impl AstDump for ast::AgentBody {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Source(body) => {
                f.node("AgentSourceBody", body.span, Vec::new(), |f| body.dump(f));
            }
            Self::Decl { semicolon_span } => {
                f.line("AgentDeclBody", *semicolon_span, Vec::new());
            }
            Self::Error(span) => {
                f.line("ErrorAgentBody", *span, Vec::new());
            }
        }
    }
}

impl AstDump for ast::TopLevelLetDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node(
            "TopLevelLetDecl",
            self.span,
            name_fields(f, &self.name),
            |f| {
                if let Some(type_annotation) = &self.type_annotation {
                    f.node("TypeAnnotation", type_annotation.span(), Vec::new(), |f| {
                        type_annotation.dump(f);
                    });
                }
                self.value.dump(f);
            },
        );
    }
}

impl AstDump for ast::MemoryAccess {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = vec![format!(
            "kind={}",
            match self.kind {
                MemoryAccessKind::Read => "read",
                MemoryAccessKind::Write => "write",
            }
        )];
        push_path_field(f, &mut fields, "path", &self.path);
        f.line("MemoryAccess", self.span, fields);
    }
}

impl AstDump for ast::ProtocolDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("ProtocolDecl", self.span, name_fields(f, &self.name), |f| {
            for message in &self.messages {
                message.dump(f);
            }
        });
    }
}

impl AstDump for ast::ProtocolMsg {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        push_path_field(f, &mut fields, "from", &self.from);
        push_path_field(f, &mut fields, "to", &self.to);
        f.node("ProtocolMsg", self.span, fields, |f| self.payload.dump(f));
    }
}

impl AstDump for ast::Param {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("Param", self.span, name_fields(f, &self.name), |f| {
            self.ty.dump(f)
        });
    }
}

impl AstDump for TypeExpr {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Handler(handler) => handler.dump(f),
            Self::Arrow {
                effect,
                input,
                output,
                span,
            } => f.node("ArrowType", *span, Vec::new(), |f| {
                if let Some(effect) = effect {
                    effect.dump(f);
                }
                f.node("Input", input.span(), Vec::new(), |f| input.dump(f));
                f.node("Output", output.span(), Vec::new(), |f| output.dump(f));
            }),
            Self::Primitive { kind, span } => {
                f.line("PrimitiveType", *span, vec![format!("kind={kind:?}")]);
            }
            Self::Path { path, args, span } => {
                let mut fields = Vec::new();
                push_path_field(f, &mut fields, "path", path);
                f.node("PathType", *span, fields, |f| {
                    for arg in args {
                        f.node("TypeArg", arg.span(), Vec::new(), |f| arg.dump(f));
                    }
                });
            }
            Self::Record(record) => record.dump(f),
            Self::Tuple { elems, span } => f.node("TupleType", *span, Vec::new(), |f| {
                for elem in elems {
                    elem.dump(f);
                }
            }),
            Self::Refined {
                base,
                predicate,
                span,
            } => f.node("RefinedType", *span, Vec::new(), |f| {
                f.node("Base", base.span(), Vec::new(), |f| base.dump(f));
                f.node("Predicate", predicate.span(), Vec::new(), |f| {
                    predicate.dump(f)
                });
            }),
            Self::Error(span) => f.line("ErrorType", *span, Vec::new()),
        }
    }
}

impl AstDump for ast::HandlerType {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("HandlerType", self.span, Vec::new(), |f| {
            f.node("Handled", self.handled.span, Vec::new(), |f| {
                self.handled.dump(f)
            });
            match &self.produced {
                ast::HandlerProducedEffects::Infer => {
                    f.line("ProducedInfer", self.span, Vec::new());
                }
                ast::HandlerProducedEffects::Explicit(effects) => {
                    f.node("Produced", effects.span, Vec::new(), |f| effects.dump(f));
                }
            }
            if let Some(result) = &self.result {
                f.node("Result", result.span(), Vec::new(), |f| result.dump(f));
            }
        });
    }
}

impl AstDump for ast::RecordType {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("RecordType", self.span, Vec::new(), |f| {
            for field in &self.fields {
                field.dump(f);
            }
        });
    }
}

impl AstDump for ast::FieldDecl {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = name_fields(f, &self.name);
        if let Some(visibility) = self.visibility {
            fields.push(format!("visibility={visibility:?}"));
        }
        f.node("FieldDecl", self.span, fields, |f| self.ty.dump(f));
    }
}

impl AstDump for ast::EffectRow {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("EffectRow", self.span, Vec::new(), |f| {
            for effect in &self.effects {
                effect.dump(f);
            }
        });
    }
}

impl AstDump for ast::GenericArg {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Type(ty) => {
                f.node("TypeArg", ty.span(), Vec::new(), |f| ty.dump(f));
            }
            Self::EffectRow(row) => {
                f.node("EffectRowArg", row.span, Vec::new(), |f| row.dump(f));
            }
            Self::Wildcard { span } => {
                f.line("WildcardArg", *span, Vec::new());
            }
        }
    }
}

impl AstDump for ast::EffectRef {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        push_path_field(f, &mut fields, "path", &self.path);
        f.node("EffectRef", self.span, fields, |f| {
            for arg in &self.args {
                arg.dump(f);
            }
        });
    }
}

impl AstDump for ast::EffectArg {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Type(ty) => f.node("EffectTypeArg", ty.span(), Vec::new(), |f| ty.dump(f)),
            Self::Path(path) => f.line(
                "EffectPathArg",
                path.span,
                path_field(f, "path", path).into_iter().collect(),
            ),
            Self::Wildcard { span } => f.line("EffectWildcardArg", *span, Vec::new()),
            Self::String { value, span } => f.line(
                "EffectStringArg",
                *span,
                token_debug_field(f, "value", value),
            ),
            Self::Int { text, span } => f.line("EffectIntArg", *span, token_text_field(f, text)),
        }
    }
}

impl AstDump for ast::Block {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("Block", self.span, Vec::new(), |f| {
            for stmt in &self.stmts {
                stmt.dump(f);
            }
            if let Some(expr) = &self.final_expr {
                f.node("FinalExpr", expr.span(), Vec::new(), |f| expr.dump(f));
            }
        });
    }
}

impl AstDump for Stmt {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Let(stmt) => f.node("LetStmt", stmt.span, Vec::new(), |f| {
                stmt.pattern.dump(f);
                if let Some(ty) = &stmt.ty {
                    f.node("Type", ty.span(), Vec::new(), |f| ty.dump(f));
                }
                f.node("Value", stmt.value.span(), Vec::new(), |f| {
                    stmt.value.dump(f)
                });
            }),
            Self::Var(stmt) => f.node("VarStmt", stmt.span, Vec::new(), |f| {
                stmt.pattern.dump(f);
                if let Some(ty) = &stmt.ty {
                    f.node("Type", ty.span(), Vec::new(), |f| ty.dump(f));
                }
                f.node("Value", stmt.value.span(), Vec::new(), |f| {
                    stmt.value.dump(f)
                });
            }),
            Self::Assign(stmt) => f.node("AssignStmt", stmt.span, Vec::new(), |f| {
                f.node("Target", stmt.target.span(), Vec::new(), |f| {
                    stmt.target.dump(f)
                });
                f.node("Value", stmt.value.span(), Vec::new(), |f| {
                    stmt.value.dump(f)
                });
            }),
            Self::If(stmt) => f.node("IfStmt", stmt.span, Vec::new(), |f| {
                f.node("Condition", stmt.condition.span(), Vec::new(), |f| {
                    stmt.condition.dump(f)
                });
                stmt.then_branch.dump(f);
                if let Some(else_branch) = &stmt.else_branch {
                    else_branch.dump(f);
                }
            }),
            Self::Match(stmt) => f.node("MatchStmt", stmt.span, Vec::new(), |f| {
                f.node("Scrutinee", stmt.scrutinee.span(), Vec::new(), |f| {
                    stmt.scrutinee.dump(f)
                });
                for arm in &stmt.arms {
                    arm.dump(f);
                }
            }),
            Self::For(stmt) => f.node("ForStmt", stmt.span, Vec::new(), |f| {
                stmt.pattern.dump(f);
                f.node("Iter", stmt.iter.span(), Vec::new(), |f| stmt.iter.dump(f));
                for limit in &stmt.limits {
                    f.node("Limit", limit.span(), Vec::new(), |f| limit.dump(f));
                }
                stmt.body.dump(f);
            }),
            Self::While(stmt) => f.node("WhileStmt", stmt.span, Vec::new(), |f| {
                f.node("Condition", stmt.condition.span(), Vec::new(), |f| {
                    stmt.condition.dump(f)
                });
                for limit in &stmt.limits {
                    f.node("Limit", limit.span(), Vec::new(), |f| limit.dump(f));
                }
                stmt.body.dump(f);
            }),
            Self::Retry(stmt) => f.node("RetryStmt", stmt.span, Vec::new(), |f| {
                for limit in &stmt.limits {
                    f.node("Limit", limit.span(), Vec::new(), |f| limit.dump(f));
                }
                stmt.body.dump(f);
            }),
            Self::Resume(stmt) => f.node("ResumeStmt", stmt.span, Vec::new(), |f| {
                if let Some(value) = &stmt.value {
                    value.dump(f);
                }
            }),
            Self::Finish(stmt) => f.node("FinishStmt", stmt.span, Vec::new(), |f| {
                stmt.value.dump(f);
            }),
            Self::Return(stmt) => f.node("ReturnStmt", stmt.span, Vec::new(), |f| {
                if let Some(value) = &stmt.value {
                    value.dump(f);
                }
            }),
            Self::Break(span) => f.line("BreakStmt", *span, Vec::new()),
            Self::Continue(span) => f.line("ContinueStmt", *span, Vec::new()),
            Self::Expr(stmt) => f.node("ExprStmt", stmt.span, Vec::new(), |f| stmt.expr.dump(f)),
            Self::Error(span) => f.line("ErrorStmt", *span, Vec::new()),
        }
    }
}

impl AstDump for ElseBranch {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::If(stmt) => f.node("ElseIf", stmt.span, Vec::new(), |f| {
                Stmt::If(*stmt.clone()).dump(f);
            }),
            Self::Block(block) => f.node("ElseBlock", block.span, Vec::new(), |f| block.dump(f)),
        }
    }
}

impl AstDump for ast::MatchArm {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("MatchArm", self.span, Vec::new(), |f| {
            self.pattern.dump(f);
            self.body.dump(f);
        });
    }
}

impl AstDump for MatchArmBody {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Expr(expr) => f.node("ArmExpr", expr.span(), Vec::new(), |f| expr.dump(f)),
            Self::Block(block) => f.node("ArmBlock", block.span, Vec::new(), |f| block.dump(f)),
        }
    }
}

impl AstDump for Expr {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Literal(lit) => lit.dump(f),
            Self::Path(path) => f.line(
                "PathExpr",
                path.span,
                path_field(f, "path", path).into_iter().collect(),
            ),
            Self::Record(record) => f.node(
                "RecordExpr",
                record.span,
                optional_path_fields(f, "path", record.path.as_ref()),
                |f| {
                    for field in &record.fields {
                        field.dump(f);
                    }
                },
            ),
            Self::EmptyRecordOrMap { span } => f.line("EmptyRecordOrMapExpr", *span, Vec::new()),
            Self::Tuple { elems, span } => f.node("TupleExpr", *span, Vec::new(), |f| {
                for elem in elems {
                    elem.dump(f);
                }
            }),
            Self::Array { elems, span } => f.node("ArrayExpr", *span, Vec::new(), |f| {
                for elem in elems {
                    elem.dump(f);
                }
            }),
            Self::List { elems, span } => f.node("ListExpr", *span, Vec::new(), |f| {
                for elem in elems {
                    elem.dump(f);
                }
            }),
            Self::ListCons { head, tail, span } => f.node("ListConsExpr", *span, Vec::new(), |f| {
                f.node("Head", head.span(), Vec::new(), |f| head.dump(f));
                f.node("Tail", tail.span(), Vec::new(), |f| tail.dump(f));
            }),
            Self::EmptySequence { span } => f.line("EmptySequenceExpr", *span, Vec::new()),
            Self::Map(map) => f.node("MapExpr", map.span, Vec::new(), |f| {
                for entry in &map.entries {
                    f.node("MapEntry", entry.span, Vec::new(), |f| {
                        f.node("Key", entry.key.span(), Vec::new(), |f| entry.key.dump(f));
                        f.node("Value", entry.value.span(), Vec::new(), |f| {
                            entry.value.dump(f)
                        });
                    });
                }
            }),
            Self::Set { elems, span } => f.node("SetExpr", *span, Vec::new(), |f| {
                for elem in elems {
                    elem.dump(f);
                }
            }),
            Self::Range(range) => {
                let bounds = match range.bounds {
                    crate::ast::RangeBounds::ClosedOpen => "closed_open",
                    crate::ast::RangeBounds::OpenClosed => "open_closed",
                };
                f.node(
                    "RangeExpr",
                    range.span,
                    vec![format!("bounds={bounds}")],
                    |f| {
                        f.node("Start", range.start.span(), Vec::new(), |f| {
                            range.start.dump(f)
                        });
                        f.node("End", range.end.span(), Vec::new(), |f| range.end.dump(f));
                    },
                );
            }
            Self::Call(call) => f.node("CallExpr", call.span, Vec::new(), |f| {
                f.node("Callee", call.callee.span(), Vec::new(), |f| {
                    call.callee.dump(f)
                });
                for arg in &call.generic_args {
                    arg.dump(f);
                }
                for arg in &call.args {
                    arg.dump(f);
                }
            }),
            Self::MethodCall(call) => f.node(
                "MethodCallExpr",
                call.span,
                name_label_fields(f, "method", &call.method),
                |f| {
                    f.node("Receiver", call.receiver.span(), Vec::new(), |f| {
                        call.receiver.dump(f)
                    });
                    for arg in &call.generic_args {
                        arg.dump(f);
                    }
                    for arg in &call.args {
                        arg.dump(f);
                    }
                },
            ),
            Self::SpecMethodCall(call) => {
                let mut fields = name_label_fields(f, "method", &call.method);
                push_path_field(f, &mut fields, "spec", &call.spec_path);
                f.node("SpecMethodCallExpr", call.span, fields, |f| {
                    f.node("Receiver", call.receiver.span(), Vec::new(), |f| {
                        call.receiver.dump(f)
                    });
                    for arg in &call.spec_args {
                        f.node("SpecTypeArg", arg.span(), Vec::new(), |f| arg.dump(f));
                    }
                    for arg in &call.args {
                        arg.dump(f);
                    }
                });
            }
            Self::Perform(perform) => {
                let mut fields = Vec::new();
                push_name_field(f, &mut fields, "action", Some(&perform.action));
                f.node("PerformExpr", perform.span, fields, |f| {
                    perform.effect.dump(f);
                    for arg in &perform.generic_args {
                        arg.dump(f);
                    }
                    for arg in &perform.args {
                        arg.dump(f);
                    }
                });
            }
            Self::Handle(handle) => f.node("HandleExpr", handle.span, Vec::new(), |f| {
                f.node("Body", handle.body.span(), Vec::new(), |f| {
                    handle.body.dump(f)
                });
                handle.handler.dump(f);
            }),
            Self::Handler(handler) => f.node("HandlerExpr", handler.span, Vec::new(), |f| {
                handler.block.dump(f);
            }),
            Self::StageCompose(compose) => {
                f.node("StageComposeExpr", compose.span, Vec::new(), |f| {
                    for stage in &compose.stages {
                        stage.dump(f);
                    }
                })
            }
            Self::Pipeline(pipeline) => f.node("PipelineExpr", pipeline.span, Vec::new(), |f| {
                f.node("Input", pipeline.input.span(), Vec::new(), |f| {
                    pipeline.input.dump(f)
                });
                for stage in &pipeline.stages {
                    stage.dump(f);
                }
            }),
            Self::Field(field) => f.node(
                "FieldExpr",
                field.span,
                name_label_fields(f, "field", &field.field),
                |f| {
                    f.node("Receiver", field.receiver.span(), Vec::new(), |f| {
                        field.receiver.dump(f)
                    });
                },
            ),
            Self::Index(index) => f.node("IndexExpr", index.span, Vec::new(), |f| {
                f.node("Receiver", index.receiver.span(), Vec::new(), |f| {
                    index.receiver.dump(f)
                });
                f.node("Index", index.index.span(), Vec::new(), |f| {
                    index.index.dump(f)
                });
            }),
            Self::Slice(slice) => {
                let bounds = match slice.bounds {
                    crate::ast::RangeBounds::ClosedOpen => "closed_open",
                    crate::ast::RangeBounds::OpenClosed => "open_closed",
                };
                f.node(
                    "SliceExpr",
                    slice.span,
                    vec![format!("bounds={bounds}")],
                    |f| {
                        f.node("Receiver", slice.receiver.span(), Vec::new(), |f| {
                            slice.receiver.dump(f)
                        });
                        f.node("Start", slice.start.span(), Vec::new(), |f| {
                            slice.start.dump(f)
                        });
                        f.node("End", slice.end.span(), Vec::new(), |f| slice.end.dump(f));
                    },
                );
            }
            Self::Try(try_expr) => f.node("TryExpr", try_expr.span, Vec::new(), |f| {
                f.node("Expr", try_expr.expr.span(), Vec::new(), |f| {
                    try_expr.expr.dump(f)
                });
            }),
            Self::Unary(unary) => f.node(
                "UnaryExpr",
                unary.span,
                vec![format!("op={}", unary_op_text(unary.op))],
                |f| {
                    unary.expr.dump(f);
                },
            ),
            Self::Binary(binary) => f.node(
                "BinaryExpr",
                binary.span,
                vec![format!("op={}", binary_op_text(binary.op))],
                |f| {
                    f.node("Lhs", binary.lhs.span(), Vec::new(), |f| binary.lhs.dump(f));
                    f.node("Rhs", binary.rhs.span(), Vec::new(), |f| binary.rhs.dump(f));
                },
            ),
            Self::If(if_expr) => f.node("IfExpr", if_expr.span, Vec::new(), |f| {
                f.node("Condition", if_expr.condition.span(), Vec::new(), |f| {
                    if_expr.condition.dump(f)
                });
                if_expr.then_branch.dump(f);
                if_expr.else_branch.dump(f);
            }),
            Self::Match(match_expr) => f.node("MatchExpr", match_expr.span, Vec::new(), |f| {
                f.node("Scrutinee", match_expr.scrutinee.span(), Vec::new(), |f| {
                    match_expr.scrutinee.dump(f);
                });
                for arm in &match_expr.arms {
                    arm.dump(f);
                }
            }),
            Self::Lambda(lambda) => f.node("LambdaExpr", lambda.span, Vec::new(), |f| {
                lambda.params.dump(f);
                lambda.body.dump(f);
            }),
            Self::Block(block) => f.node("BlockExpr", block.block.span, Vec::new(), |f| {
                block.block.dump(f)
            }),
            Self::Error(span) => f.line("ErrorExpr", *span, Vec::new()),
        }
    }
}

impl AstDump for Literal {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Bool { value, span } => {
                f.line("BoolLiteral", *span, token_value_field(f, *value))
            }
            Self::Int { text, span } => f.line("IntLiteral", *span, token_text_field(f, text)),
            Self::Float { text, span } => f.line("FloatLiteral", *span, token_text_field(f, text)),
            Self::String { value, span } => {
                f.line("StringLiteral", *span, token_debug_field(f, "value", value))
            }
            Self::Char { value, span } => {
                f.line("CharLiteral", *span, token_debug_field(f, "value", value))
            }
        }
    }
}

impl AstDump for FieldInit {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Shorthand(name) => f.line("ShorthandField", name.span, name_fields(f, name)),
            Self::Named { name, value } => {
                f.node(
                    "NamedField",
                    name.span.cover(value.span()),
                    name_fields(f, name),
                    |f| {
                        value.dump(f);
                    },
                );
            }
        }
    }
}

impl AstDump for Arg {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Positional(expr) => {
                f.node("PositionalArg", expr.span(), Vec::new(), |f| expr.dump(f))
            }
            Self::Named { name, value } => {
                f.node(
                    "NamedArg",
                    name.span.cover(value.span()),
                    name_fields(f, name),
                    |f| {
                        value.dump(f);
                    },
                );
            }
        }
    }
}

impl AstDump for ast::HandlerBlock {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("HandlerBlock", self.span, Vec::new(), |f| {
            for arm in &self.arms {
                arm.dump(f);
            }
        });
    }
}

impl AstDump for ast::HandlerArm {
    fn dump(&self, f: &mut AstDumpWriter) {
        let mut fields = Vec::new();
        push_name_field(f, &mut fields, "action", Some(&self.action));
        f.node("HandlerArm", self.span, fields, |f| {
            self.effect.dump(f);
            for arg in &self.generic_args {
                arg.dump(f);
            }
            for pattern in &self.patterns {
                pattern.dump(f);
            }
            self.body.dump(f);
        });
    }
}

impl AstDump for ast::HandlerArmBody {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Stmt(stmt) => {
                f.node("HandlerArmStmtBody", stmt.span(), Vec::new(), |f| {
                    stmt.dump(f)
                });
            }
            Self::Block(block) => block.dump(f),
        }
    }
}

impl AstDump for ast::PipelineStage {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("PipelineStage", self.span, Vec::new(), |f| {
            self.expr.dump(f);
            for limit in &self.limits {
                f.node("Limit", limit.span(), Vec::new(), |f| limit.dump(f));
            }
        });
    }
}

impl AstDump for ast::HandlerArg {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Expr(expr) => {
                f.node("HandlerExprArg", expr.span(), Vec::new(), |f| expr.dump(f));
            }
            Self::Block(block) => {
                f.node("HandlerBlockArg", block.span, Vec::new(), |f| block.dump(f));
            }
        }
    }
}

impl AstDump for LambdaParams {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Ident(name) => f.line("LambdaParam", name.span, name_fields(f, name)),
            Self::ParamList(params) => {
                let span = params
                    .first()
                    .zip(params.last())
                    .map(|(first, last)| first.span.cover(last.span))
                    .unwrap_or_else(|| Span::empty(crate::SourceId(0), crate::TextSize::ZERO));
                f.node("LambdaParams", span, Vec::new(), |f| {
                    for param in params {
                        param.dump(f);
                    }
                });
            }
        }
    }
}

impl AstDump for LambdaBody {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Expr(expr) => f.node("LambdaBodyExpr", expr.span(), Vec::new(), |f| expr.dump(f)),
            Self::Block(block) => {
                f.node("LambdaBodyBlock", block.span, Vec::new(), |f| block.dump(f))
            }
        }
    }
}

impl AstDump for Pattern {
    fn dump(&self, f: &mut AstDumpWriter) {
        match self {
            Self::Ident(name) => f.line("IdentPattern", name.span, name_fields(f, name)),
            Self::Wildcard(span) => f.line("WildcardPattern", *span, Vec::new()),
            Self::Literal(literal) => f.node("LiteralPattern", literal.span(), Vec::new(), |f| {
                literal.dump(f);
            }),
            Self::Tuple { elems, span } => f.node("TuplePattern", *span, Vec::new(), |f| {
                for elem in elems {
                    elem.dump(f);
                }
            }),
            Self::Record(record) => f.node(
                "RecordPattern",
                record.span,
                optional_path_fields(f, "path", record.path.as_ref()),
                |f| {
                    for field in &record.fields {
                        field.dump(f);
                    }
                },
            ),
            Self::Variant(variant) => {
                let mut fields = Vec::new();
                push_path_field(f, &mut fields, "path", &variant.path);
                f.node("VariantPattern", variant.span, fields, |f| {
                    for pattern in &variant.patterns {
                        pattern.dump(f);
                    }
                });
            }
            Self::Error(span) => f.line("ErrorPattern", *span, Vec::new()),
        }
    }
}

impl AstDump for ast::PatternField {
    fn dump(&self, f: &mut AstDumpWriter) {
        f.node("PatternField", self.span, name_fields(f, &self.name), |f| {
            if let Some(pattern) = &self.pattern {
                pattern.dump(f);
            }
        });
    }
}

fn name_fields(f: &AstDumpWriter, name: &ast::Name) -> Vec<String> {
    name_label_fields(f, "name", name)
}

fn name_label_fields(f: &AstDumpWriter, label: &str, name: &ast::Name) -> Vec<String> {
    f.token_field(label, &name.text).into_iter().collect()
}

fn push_name_field(
    f: &AstDumpWriter,
    fields: &mut Vec<String>,
    label: &str,
    name: Option<&ast::Name>,
) {
    if let Some(name) = name {
        if let Some(field) = f.token_field(label, &name.text) {
            fields.push(field);
        }
    }
}

fn path_field(f: &AstDumpWriter, label: &str, path: &ast::Path) -> Option<String> {
    f.options
        .include_tokens
        .then(|| format!("{label}={}", dump_atom(&path_text(path))))
}

fn push_path_field(f: &AstDumpWriter, fields: &mut Vec<String>, label: &str, path: &ast::Path) {
    if let Some(field) = path_field(f, label, path) {
        fields.push(field);
    }
}

fn optional_path_fields(f: &AstDumpWriter, label: &str, path: Option<&ast::Path>) -> Vec<String> {
    path.and_then(|path| path_field(f, label, path))
        .into_iter()
        .collect()
}

fn push_type_params(f: &AstDumpWriter, fields: &mut Vec<String>, params: &[ast::TypeParam]) {
    if !f.options.include_tokens || params.is_empty() {
        return;
    }

    let params = params
        .iter()
        .map(|param| {
            let mut text = dump_atom(&param.name.text);
            match param.kind {
                ast::TypeParamKind::Type if !param.bounds.is_empty() => {
                    let bounds = param
                        .bounds
                        .iter()
                        .map(type_param_bound_text)
                        .collect::<Vec<_>>()
                        .join("+");
                    text.push('~');
                    text.push_str(&bounds);
                }
                ast::TypeParamKind::Effect => text.push_str(" ~effect"),
                ast::TypeParamKind::Type => {}
            }
            text
        })
        .collect::<Vec<_>>()
        .join(",");
    fields.push(format!("type_params=<{params}>"));
}

fn push_spec_bounds(f: &AstDumpWriter, fields: &mut Vec<String>, bounds: &[ast::TypeParamBound]) {
    if !f.options.include_tokens || bounds.is_empty() {
        return;
    }

    let bounds = bounds
        .iter()
        .map(type_param_bound_text)
        .collect::<Vec<_>>()
        .join("+");
    fields.push(format!("bounds=<{bounds}>"));
}

fn type_param_bound_text(bound: &ast::TypeParamBound) -> String {
    let mut text = path_text(&bound.path);
    if !bound.args.is_empty() {
        text.push('<');
        text.push_str(
            &bound
                .args
                .iter()
                .map(type_expr_text)
                .collect::<Vec<_>>()
                .join(","),
        );
        text.push('>');
    }
    text
}

fn type_expr_text(ty: &ast::TypeExpr) -> String {
    match ty {
        ast::TypeExpr::Primitive { kind, .. } => primitive_type_name(*kind).to_owned(),
        ast::TypeExpr::Path { path, args, .. } => {
            let mut text = path_text(path);
            if !args.is_empty() {
                text.push('<');
                text.push_str(
                    &args
                        .iter()
                        .map(type_expr_text)
                        .collect::<Vec<_>>()
                        .join(","),
                );
                text.push('>');
            }
            text
        }
        _ => "<type>".to_owned(),
    }
}

fn primitive_type_name(kind: ast::PrimitiveType) -> &'static str {
    match kind {
        ast::PrimitiveType::Bool => "bool",
        ast::PrimitiveType::I8 => "i8",
        ast::PrimitiveType::I16 => "i16",
        ast::PrimitiveType::I32 => "i32",
        ast::PrimitiveType::I64 => "i64",
        ast::PrimitiveType::I128 => "i128",
        ast::PrimitiveType::Isize => "isize",
        ast::PrimitiveType::U8 => "u8",
        ast::PrimitiveType::U16 => "u16",
        ast::PrimitiveType::U32 => "u32",
        ast::PrimitiveType::U64 => "u64",
        ast::PrimitiveType::U128 => "u128",
        ast::PrimitiveType::Usize => "usize",
        ast::PrimitiveType::F32 => "f32",
        ast::PrimitiveType::F64 => "f64",
        ast::PrimitiveType::String => "string",
        ast::PrimitiveType::Bytes => "bytes",
        ast::PrimitiveType::Char => "char",
        ast::PrimitiveType::Unit => "unit",
        ast::PrimitiveType::Never => "never",
    }
}

fn token_value_field(f: &AstDumpWriter, value: bool) -> Vec<String> {
    f.options
        .include_tokens
        .then(|| format!("value={value}"))
        .into_iter()
        .collect()
}

fn token_text_field(f: &AstDumpWriter, text: &str) -> Vec<String> {
    f.token_field("text", text).into_iter().collect()
}

fn token_debug_field<T: std::fmt::Debug>(f: &AstDumpWriter, label: &str, value: T) -> Vec<String> {
    f.options
        .include_tokens
        .then(|| format!("{label}={value:?}"))
        .into_iter()
        .collect()
}

fn path_text(path: &ast::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn dump_atom(text: &str) -> String {
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        && !text.is_empty()
    {
        text.to_owned()
    } else {
        format!("{text:?}")
    }
}

fn format_span(span: Span) -> String {
    format!(
        "@{}..{}",
        span.range.start.to_usize(),
        span.range.end.to_usize()
    )
}

fn format_range(range: TextRange) -> String {
    format!("@{}..{}", range.start.to_usize(), range.end.to_usize())
}

fn severity_text(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

fn label_style_text(style: LabelStyle) -> &'static str {
    match style {
        LabelStyle::Primary => "primary",
        LabelStyle::Secondary => "secondary",
    }
}

fn unary_op_text(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}

fn binary_op_text(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::EqEq => "==",
        BinaryOp::BangEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::AndAnd => "&&",
        BinaryOp::OrOr => "||",
    }
}

fn spec_kind_text(kind: ast::SpecKind) -> &'static str {
    match kind {
        ast::SpecKind::TypeSpec => "type",
        ast::SpecKind::CallableSpec => "callable",
        ast::SpecKind::TraceSpec => "trace",
    }
}
