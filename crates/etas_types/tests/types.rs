use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_hir::{HirItem, lower_program};
use etas_types::{
    Assignable, EffectActionArgKind, EffectActionSignature, FieldType, FlowSignature, FlowType,
    PrimitiveType, RecordType, SymbolTypeFact, Type, TypeFacts, TypeId, TypeInterner, TypeRelation,
    TypeUnifier, TypeVarId, check_program, check_top_level_items,
};

#[test]
fn primitive_types_have_source_names_and_stable_ids() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let string_again = types.primitive(PrimitiveType::String);
    let unit = types.primitive(PrimitiveType::Unit);

    assert_eq!(PrimitiveType::String.source_name(), "string");
    assert_eq!(string, string_again);
    assert_ne!(string, unit);
}

#[test]
fn numeric_literal_facts_preserve_each_expression_context() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> i32 {
  let narrow: i8 = 1;
  let index: usize = 1;
  let defaulted = 1;
  return defaulted;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let literal_types = hir
        .exprs
        .iter()
        .filter_map(|(expr, data)| {
            matches!(
                data,
                etas_hir::HirExpr::Literal(etas_hir::HirLiteral::Int { .. })
            )
            .then(|| output.facts.expr_types.get(&expr).copied())
            .flatten()
        })
        .filter_map(|ty| output.store.get(ty))
        .filter_map(|ty| match ty {
            Type::Primitive(primitive) => Some(*primitive),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(
        literal_types,
        std::collections::HashSet::from([
            PrimitiveType::I8,
            PrimitiveType::I32,
            PrimitiveType::USize,
        ])
    );
}

#[test]
fn check_program_validates_integer_radices_widths_and_signed_minimum() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> i8 {
  let decimal: u8 = 255;
  let hex: u8 = 0xff;
  let binary: u8 = 0b1111_1111;
  let octal: u8 = 0o377;
  let machine_word: usize = 18_446_744_073_709_551_615;
  return -128;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_numeric_literals_outside_their_checked_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(7),
        None,
        r#"
flow main() -> unit {
  let unsigned_overflow: u8 = 256;
  let signed_positive_overflow: i8 = 128;
  let signed_negative_overflow: i8 = -129;
  let float32_overflow: f32 = 1e39;
  let float64_overflow: f64 = 1e309;
  let float64_underflow: f64 = 1e-400;
  return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);
    let range_errors = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && (diagnostic.message.contains("outside the")
                    || diagnostic.message.contains("not finite"))
        })
        .collect::<Vec<_>>();

    assert_eq!(range_errors.len(), 6, "{:?}", output.diagnostics);
    assert!(
        range_errors
            .iter()
            .all(|diagnostic| diagnostic.primary.span.source == etas_core::SourceId(7))
    );
}

#[test]
fn unifier_solves_variables_and_detects_occurs_check() {
    let mut types = TypeInterner::new();
    let var = types.intern(Type::Var(TypeVarId(0)));
    let string = types.primitive(PrimitiveType::String);
    let list_var = types.intern(Type::List(var));
    let store = types.store();

    let mut unifier = TypeUnifier::new(store);
    unifier.unify(var, string).expect("var should bind");
    assert_eq!(unifier.substitution().get(TypeVarId(0)), Some(string));

    let mut recursive = TypeUnifier::new(store);
    assert!(recursive.unify(var, list_var).is_err());
}

#[test]
fn assignability_allows_never_and_structural_function_match() {
    let mut types = TypeInterner::new();
    let never = types.primitive(PrimitiveType::Never);
    let string = types.primitive(PrimitiveType::String);
    let fn_a = types.intern(Type::Function(FlowType {
        input: vec![string],
        output: string,
        effects: None,
    }));
    let fn_b = types.intern(Type::Function(FlowType {
        input: vec![string],
        output: string,
        effects: None,
    }));

    let relation = TypeRelation::new(types.store());
    relation
        .assignable(never, string)
        .expect("never coerces to expected type");
    relation
        .assignable(fn_a, fn_b)
        .expect("matching function types assign");
}

#[test]
fn type_facts_store_symbol_and_item_contracts() {
    let mut types = TypeInterner::new();
    let string = types.primitive(PrimitiveType::String);
    let record = types.intern(Type::Record(RecordType {
        fields: vec![FieldType {
            name: "title".to_owned(),
            ty: string,
        }],
    }));

    let mut facts = TypeFacts::default();
    facts
        .symbol_types
        .insert(etas_hir::SymbolId(0), SymbolTypeFact::Field { ty: string });
    facts.item_signatures.insert(
        etas_hir::HirItemId(0),
        etas_types::ItemSignature::Flow(FlowSignature {
            params: vec![record],
            output: string,
            effects: None,
            requested_actions: None,
        }),
    );

    assert!(matches!(
        facts.symbol_types.get(&etas_hir::SymbolId(0)),
        Some(SymbolTypeFact::Field { ty }) if *ty == string
    ));
    assert!(facts.item_signatures.contains_key(&etas_hir::HirItemId(0)));
}

#[test]
fn check_program_records_flow_signature_and_return_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        "flow echo(x: string) -> string { return x; }",
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (flow_id, flow) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Flow(flow) => Some((id, flow)),
            _ => None,
        })
        .expect("flow should lower");
    let signature = match output.facts.item_signatures.get(&flow_id) {
        Some(etas_types::ItemSignature::Flow(signature)) => signature,
        other => panic!("expected flow signature, got {other:?}"),
    };
    assert_eq!(signature.params.len(), 1);
    assert!(matches!(
        output.store.get(signature.output),
        Some(Type::Primitive(PrimitiveType::String))
    ));
    assert!(matches!(
        output.facts.symbol_types.get(&flow.symbol),
        Some(SymbolTypeFact::Flow { .. })
    ));
}

#[test]
fn effect_row_rejects_runtime_value_action_arguments() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Workspace {
    action read(path: string) -> string;
}

flow bad(path: string) -> string ![Workspace.read<path>] {
    return perform Workspace.read(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidEffectArgument)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn effect_row_rejects_unresolved_static_type_selector() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Workspace {
    action read<R>(path: string) -> string;
}

flow bad(path: string) -> string ![Workspace.read<MissingRegion>] {
    return perform Workspace.read<MissingRegion>(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidEffectArgument)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn effect_row_invalid_selector_does_not_materialize_wildcard_effect_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Workspace {
    action read<R>(path: string) -> string;
}

flow bad(path: string) -> string ![Workspace.read<MissingRegion>] {
    return perform Workspace.read<MissingRegion>(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    let (item_id, _) = hir
        .items
        .iter()
        .find(|(_, item)| {
            let HirItem::Flow(flow) = item else {
                return false;
            };
            hir.symbols
                .get(flow.symbol)
                .is_some_and(|symbol| symbol.name == "bad")
        })
        .expect("bad flow should lower");
    let signature = match output.facts.item_signatures.get(&item_id) {
        Some(etas_types::ItemSignature::Flow(signature)) => signature,
        other => panic!("expected flow signature, got {other:?}"),
    };
    let effects = signature.effects.as_ref().expect("effect row should lower");
    assert!(
        effects.effects.is_empty(),
        "invalid selector must not degrade into wildcard effect facts: {effects:?}"
    );
}

#[test]
fn effect_row_accepts_declared_static_type_selector_and_wildcard() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Region;

effect Workspace {
    action read<R>(path: string) -> string;
}

flow concrete(path: string) -> string ![Workspace.read<Region>] {
    return perform Workspace.read<Region>(path);
}

flow wildcard(path: string) -> string ![Workspace.read<_>] {
    return perform Workspace.read(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn effect_row_accepts_flow_type_param_as_static_action_selector() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Workspace {
    action read<R>(path: string) -> string;
}

flow read_under<R>(path: string) -> string ![Workspace.read<R>] {
    return perform Workspace.read<R>(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn perform_rejects_extra_runtime_selector_when_action_declares_none() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Request = {
    host: string,
};

effect Probe {
    action request(request: Request) -> string;
}

flow bad(request: Request) -> string {
    return perform Probe.request<request.host>(request);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidEffectArgument)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn source_action_selectors_come_from_declared_selector_params_not_payload_types() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Workspace {
    action read<R>(path: ProjectPath) -> string;
    action write(path: string, body: string) -> unit;
}

spec ReadablePath<R>;
type ProjectPath;
type ProjectRegion;

impl ProjectPath ~ ReadablePath<ProjectRegion>;
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let actions = hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Effect(effect) => Some(effect.body.actions()),
            _ => None,
        })
        .flatten()
        .map(|action| {
            output
                .facts
                .action_signatures
                .get(&action.symbol)
                .expect("action signature")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        actions.first().expect("read action").effect_args,
        vec![EffectActionArgKind::Type]
    );
    assert!(
        actions.get(1).expect("write action").effect_args.is_empty(),
        "payload params must not become static action selectors"
    );
}

#[test]
fn source_action_payload_generic_is_not_static_selector() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec WorkspaceRegion;
spec ReadablePath<R>;

type ReportsRoot;
type ReportPath;

impl ReportsRoot ~ WorkspaceRegion;
impl ReportPath ~ ReadablePath<ReportsRoot>;

effect Workspace {
    action read<R ~ WorkspaceRegion, P ~ ReadablePath<R>>(path: P) -> string;
}

flow read_under<R ~ WorkspaceRegion, P ~ ReadablePath<R>>(path: P) -> string ![Workspace.read<R, P>] {
    return perform Workspace.read<R, P>(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let action = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Effect(effect) => effect.body.actions().first(),
            _ => None,
        })
        .expect("action");
    let signature = output
        .facts
        .action_signatures
        .get(&action.symbol)
        .expect("action signature");
    assert_eq!(
        signature.effect_args,
        vec![EffectActionArgKind::Type, EffectActionArgKind::Type]
    );
    assert_eq!(
        signature.selector_param_names,
        vec!["R".to_owned(), "P".to_owned()]
    );
}

#[test]
fn source_action_selector_arity_keeps_payload_generic_params() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec BrowserSession;
spec DeliverableAddress;

type Url;
type PageSnapshot;
type EmailAccount;
type EmailReceipt;
type BrowserSessionRef;
type EmailAddress;
type EmailDraft<A ~ DeliverableAddress> = {
    to: A,
};

impl BrowserSessionRef ~ BrowserSession;
impl EmailAddress ~ DeliverableAddress;

effect EdkBrowser {
    action navigate<S ~ BrowserSession>(session: S, url: Url) -> PageSnapshot;
}

effect EdkEmail {
    action send<A ~ DeliverableAddress>(account: EmailAccount, draft: EmailDraft<A>) -> EmailReceipt;
}

flow navigate<S ~ BrowserSession>(session: S, url: Url) -> PageSnapshot ![EdkBrowser.navigate<S>] {
    return perform EdkBrowser.navigate<S>(session, url);
}

flow send<A ~ DeliverableAddress>(account: EmailAccount, draft: EmailDraft<A>) -> EmailReceipt ![EdkEmail.send<A>] {
    return perform EdkEmail.send<A>(account, draft);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut signatures = hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Effect(effect) => Some(effect.body.actions()),
            _ => None,
        })
        .flatten()
        .map(|action| {
            let name = hir
                .symbols
                .get(action.symbol)
                .expect("action symbol")
                .name
                .clone();
            let signature = output
                .facts
                .action_signatures
                .get(&action.symbol)
                .expect("action signature");
            (name, signature)
        })
        .collect::<Vec<_>>();
    signatures.sort_by(|(lhs, _), (rhs, _)| lhs.cmp(rhs));

    for (name, signature) in signatures {
        assert_eq!(
            signature.effect_args,
            vec![EffectActionArgKind::Type],
            "{name}"
        );
        assert_eq!(signature.selector_param_names.len(), 1, "{name}");
    }
}

#[test]
fn check_program_records_effect_action_signature_and_never_return() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Error {
    action raise(message: string) -> never;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (_, action) = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Effect(effect) => effect.body.actions().first().map(|action| (effect, action)),
            _ => None,
        })
        .expect("effect action should lower");
    let signature = output
        .facts
        .action_signatures
        .get(&action.symbol)
        .expect("action signature should be recorded");

    assert!(signature.returns_never);
    assert_eq!(signature.params.len(), 1);
    assert!(matches!(
        output.facts.symbol_types.get(&action.symbol),
        Some(SymbolTypeFact::EffectAction {
            signature: EffectActionSignature {
                returns_never: true,
                ..
            }
        })
    ));
    assert!(matches!(
        output.store.get(signature.output),
        Some(Type::Primitive(PrimitiveType::Never))
    ));
}

#[test]
fn check_program_records_impl_effect_action_signature() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Error;

impl Error {
    action raise(message: string) -> never;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let action = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Impl(impl_decl) => impl_decl.items.iter().find_map(|item| match item {
                etas_hir::HirImplItem::Action(action) => Some(action),
                _ => None,
            }),
            _ => None,
        })
        .expect("impl action should lower");
    let signature = output
        .facts
        .action_signatures
        .get(&action.symbol)
        .expect("impl action signature should be recorded");
    assert!(signature.returns_never);
}

#[test]
fn check_program_rejects_mixed_impl_item_kinds() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Box = string;
effect Approval;

impl Box {
    action invalid() -> unit;
}

impl Approval {
    flow invalid() -> unit {
        return;
    }
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    let invalid_impls = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplMemberKind)
        })
        .count();
    assert_eq!(invalid_impls, 2, "{:?}", output.diagnostics);
}

#[test]
fn build_signature_facts_rejects_action_inside_type_impl() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Writer = {
    name: string,
}

impl Writer {
    action save(name: string) -> unit;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = etas_types::build_signature_facts(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplMemberKind)
                && diagnostic.message.contains("flow methods")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_reports_return_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        "flow wrong() -> string { return true; }",
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_record_construction_and_field_access() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Row = { title: string, count: i64 };

flow main() -> string {
  let row = Row { title = "demo", count = 1 };
  return (row).title;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let field_ty = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Field { field, .. } if field == "title" => {
                output.facts.expr_types.get(&id).copied()
            }
            _ => None,
        })
        .expect("field access should have a type fact");
    assert!(matches!(
        output.store.get(field_ty),
        Some(Type::Primitive(PrimitiveType::String))
    ));
}

#[test]
fn nominal_record_constructor_accepts_alias_to_applied_nominal_field() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Region;
type Root;
impl Root ~ Region;

type Path<R ~ Region> = string;
alias RootPath = Path<Root>;

type WorkspaceError = {
  path: RootPath,
  message: string,
};

flow make_error(path: RootPath, message: string) -> WorkspaceError {
  return WorkspaceError {
    path = path,
    message = message,
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn generic_nominal_alias_satisfies_parameterized_spec_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Root;
spec Region;
impl Root ~ Region;

type Path<R ~ Region> = string;
alias RootPath = Path<Root>;

spec Readable<R ~ Region>;
impl RootPath ~ Readable<Root>;

flow read<R ~ Region, P ~ Readable<R>>(p: P) -> unit {
  return;
}

flow call(path: RootPath) -> unit {
  return read<Root, RootPath>(path);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_missing_record_field() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Row = { title: string };

flow main() -> string {
  let row = Row { title = "demo" };
  return (row).missing;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::UnknownField)
    }));
}

#[test]
fn check_program_reports_duplicate_record_expression_field() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Row = { title: string };

flow main() -> unit {
  let row = Row { title = "demo", title = "again" };
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::DuplicateField)
    }));
}

#[test]
fn check_program_reports_duplicate_record_type_field() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Row = { title: string, title: string };

flow main() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::DuplicateField)
    }));
}

#[test]
fn check_program_reports_generic_constructor_arity_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(xs: List, ys: Map<string>, prompt: Prompt<string>) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    let arity_errors = output
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::ArityMismatch)
        })
        .count();
    assert_eq!(arity_errors, 3, "{:?}", output.diagnostics);
}

#[test]
fn check_program_unifies_record_annotations_by_field_name() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Row = { title: string, count: i64 };

flow main() -> unit {
  let row: { count: i64, title: string } = Row { title = "demo", count = true };
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::Mismatch)
    }));
}

#[test]
fn check_program_discards_if_and_match_statement_values() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(flag: bool, input: string) -> unit {
  if flag {
    1
  } else {
    2
  }
  match input {
    _ => input,
  }
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let flow = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow),
            _ => None,
        })
        .expect("flow should lower");
    let body = &hir.blocks[flow.body.block()];
    for stmt in &body.stmts[0..2] {
        let ty = output
            .facts
            .stmt_types
            .get(stmt)
            .expect("statement should have a type fact");
        assert!(matches!(
            output.store.get(*ty),
            Some(Type::Primitive(PrimitiveType::Unit))
        ));
    }
}

#[test]
fn check_program_reports_match_branch_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(input: string) -> unit {
  match input {
    _ => input,
    value => true,
  }
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::BranchTypeMismatch)
    }));
}

#[test]
fn check_program_types_perform_expression_from_action_signature() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(input: string) -> string {
  let decision = perform Approval.request(input);
  return decision;
}

effect Approval {
  action request(message: string) -> string;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let perform_ty = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Perform { .. } => output.facts.expr_types.get(&id).copied(),
            _ => None,
        })
        .expect("perform expression should have a type fact");
    assert!(matches!(
        output.store.get(perform_ty),
        Some(Type::Primitive(PrimitiveType::String))
    ));
}

#[test]
fn check_program_types_first_class_handler_values() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request(message: string) -> bool;
}

let AutoApproval: ![Approval => []] = handler {
  Approval.request(message) => {
    resume true;
  }
};

flow choose(approval_handler: ![Approval => []]) -> ![Approval => []] {
  return approval_handler;
}

flow main() -> bool {
  let selected = choose(AutoApproval);
  return handle perform Approval.request("ship") with selected;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let handler_type = hir
        .items
        .iter()
        .find_map(|(_, item)| match item {
            HirItem::TopLevelLet(item)
                if hir
                    .symbols
                    .get(item.symbol)
                    .is_some_and(|symbol| symbol.name == "AutoApproval") =>
            {
                output.facts.symbol_types.get(&item.symbol)
            }
            _ => None,
        })
        .expect("AutoApproval should have a symbol type");
    assert!(matches!(
        handler_type,
        SymbolTypeFact::TopLevelLet { ty, .. }
            if matches!(output.store.get(*ty), Some(Type::Handler(handler)) if handler.handled.effects.len() == 1)
    ));
}

#[test]
fn check_program_reports_handler_action_pattern_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request(message: string) -> bool;
}

let BadApproval: ![Approval] = handler {
  Approval.request(true) => {
    resume false;
  }
};
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_reports_handler_resume_payload_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request() -> bool;
}

let BadApproval: ![Approval] = handler {
  Approval.request() => {
    resume "bad";
  }
};
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_for_result_fallback_handler() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Error {
  action stop() -> never;
}

let StopFallback: ![Error for string] = handler {
  Error.stop() => {
    finish "fallback";
  }
};

flow main() -> string {
  return handle perform Error.stop() with StopFallback;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_handler_fallback_result_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request() -> bool;
}

let BadFallback: ![Approval for string] = handler {
  Approval.request() => {
    finish true;
  }
};
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::FinishTypeMismatch)
    }));
}

#[test]
fn check_program_rejects_implicit_handler_fallback_expression() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Error {
  action stop() -> never;
}

let StopFallback: ![Error for string] = handler {
  Error.stop() => {
    "fallback"
  }
};
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::HandlerCompletionRequired)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_resume_for_never_effect_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Error {
  action stop() -> never;
}

let StopFallback: ![Error for string] = handler {
  Error.stop() => {
    resume "bad";
  }
};
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_top_level_items(&hir, check_program(&hir));

    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("return type is never")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_reports_perform_argument_count_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request(message: string) -> string;
}

flow main() -> string {
  return perform Approval.request();
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::WrongArgumentCount)
    }));
}

#[test]
fn check_program_reports_perform_argument_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Approval {
  action request(message: string) -> string;
}

flow main() -> string {
  return perform Approval.request(true);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_bool_not_and_numeric_negation() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(flag: bool, count: i64, ratio: f64) -> bool {
  let negative_count = -count;
  let negative_ratio = -ratio;
  return !flag;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_unary_negation_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(input: string) -> unit {
  let wrong = -input;
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_try_expr_as_result_value() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow capture(value: string) -> Result<string, string> {
  return value?;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let try_types = hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Unary { .. } => None,
            etas_hir::HirExpr::Try { .. } => output.facts.expr_types.get(&id).copied(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(try_types.len(), 1);
    for ty in try_types {
        let Some(Type::Result { ok, err }) = output.store.get(ty) else {
            panic!("try expression should type as Result");
        };
        assert!(matches!(
            output.store.get(*ok),
            Some(Type::Primitive(PrimitiveType::String))
        ));
        assert!(matches!(
            output.store.get(*err),
            Some(Type::Primitive(PrimitiveType::String))
        ));
    }
}

#[test]
fn check_program_types_option_result_constructors_with_expected_types() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  let later = () => {
    return;
  };
  let maybe: Option<unit -> unit> = Some(later);
  let outcome: Result<unit -> unit, string> = Ok(later);
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let constructor_types = hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Call { callee, .. }
                if matches!(&hir.exprs[*callee], etas_hir::HirExpr::Path(path) if path.segments.last().is_some_and(|segment| segment.name == "Some" || segment.name == "Ok")) =>
            {
                output.facts.expr_types.get(&id).copied()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(constructor_types.len(), 2);
    assert!(constructor_types.iter().any(|ty| {
        matches!(
            output.store.get(*ty),
            Some(Type::Option(inner))
                if matches!(output.store.get(*inner), Some(Type::Function(_)))
        )
    }));
    assert!(constructor_types.iter().any(|ty| {
        matches!(
            output.store.get(*ty),
            Some(Type::Result { ok, err })
                if matches!(output.store.get(*ok), Some(Type::Function(_)))
                    && matches!(output.store.get(*err), Some(Type::Primitive(PrimitiveType::String)))
        )
    }));
}

#[test]
fn check_program_rejects_result_constructor_without_target_error_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  let value = Ok("done");
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_reports_try_propagation_operand_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(value: string) -> string {
  return value?;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_sequence_bytes_and_map_index_expressions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow first(xs: List<string>) -> string {
  let index: i32 = 0;
  return xs[index];
}

flow lookup(table: Map<string, i64>) -> i64 {
  return table["answer"];
}

flow char_at(text: string) -> char {
  return text[1];
}

flow byte_at(data: bytes) -> u8 {
  return data[1];
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let index_types = hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Index { .. } => {
                Some((id, output.facts.expr_types.get(&id).copied()?))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(index_types.len(), 4);
    assert!(matches!(
        output.store.get(index_types[0].1),
        Some(Type::Primitive(PrimitiveType::String))
    ));
    assert!(matches!(
        output.store.get(index_types[1].1),
        Some(Type::Primitive(PrimitiveType::I64))
    ));
    assert!(matches!(
        output.store.get(index_types[2].1),
        Some(Type::Primitive(PrimitiveType::Char))
    ));
    assert!(matches!(
        output.store.get(index_types[3].1),
        Some(Type::Primitive(PrimitiveType::U8))
    ));
    assert!(
        output
            .facts
            .checked_index_errors
            .contains_key(&index_types[0].0),
        "sequence indexing must materialize checked IndexError facts"
    );
    assert!(
        !output
            .facts
            .checked_index_errors
            .contains_key(&index_types[1].0),
        "Map lookup must not use the checked IndexError model"
    );
    assert!(
        output
            .facts
            .checked_index_errors
            .contains_key(&index_types[2].0),
        "string indexing must materialize checked IndexError facts"
    );
    assert!(
        output
            .facts
            .checked_index_errors
            .contains_key(&index_types[3].0),
        "bytes indexing must materialize checked IndexError facts"
    );
}

#[test]
fn check_program_types_indexed_alias_record_field_access() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias MatchPair = {
  left: i32,
  right: i32,
};

alias MatchResult = {
  pairs: Array<MatchPair>,
  count: i32,
};

alias Edit = {
  tag: string,
  old_index: i32,
  new_index: i32,
  value: i32,
};

alias EditScript = {
  edits: Array<Edit>,
  distance: i32,
};

alias PriorityItem = {
  value: i32,
  priority: i32,
  sequence: i32,
};

alias PriorityItemQueue = {
  items: Array<PriorityItem>,
};

alias QueueItem<T> = {
  value: T,
  priority: i32,
  sequence: i32,
};

alias PriorityQueue<T> = {
  items: Array<QueueItem<T>>,
  next_sequence: i32,
};

flow main(result: MatchResult, diff: EditScript, queue: PriorityItemQueue, generic_queue: PriorityQueue<string>, index: i32) -> i32 {
  let left: i32 = result.pairs[0].left;
  let right: i32 = result.pairs[0].right;
  let tag: string = diff.edits[0].tag;
  let old_index: i32 = diff.edits[0].old_index;
  let new_index: i32 = diff.edits[0].new_index;
  let value: i32 = queue.items[index].value;
  let priority: i32 = queue.items[index].priority;
  let generic_value: string = generic_queue.items[index].value;
  return left + right + old_index + new_index + value + priority;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let field_types = hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Field { field, .. } => output
                .facts
                .expr_types
                .get(&id)
                .copied()
                .map(|ty| (field, ty)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(field_types.iter().any(|(field, ty)| {
        field.as_str() == "left"
            && matches!(
                output.store.get(*ty),
                Some(Type::Primitive(PrimitiveType::I32))
            )
    }));
    assert!(field_types.iter().any(|(field, ty)| {
        field.as_str() == "tag"
            && matches!(
                output.store.get(*ty),
                Some(Type::Primitive(PrimitiveType::String))
            )
    }));
    assert!(field_types.iter().any(|(field, ty)| {
        field.as_str() == "value"
            && matches!(
                output.store.get(*ty),
                Some(Type::Primitive(PrimitiveType::I32))
            )
    }));
    assert!(field_types.iter().any(|(field, ty)| {
        field.as_str() == "value"
            && matches!(
                output.store.get(*ty),
                Some(Type::Primitive(PrimitiveType::String))
            )
    }));
}

#[test]
fn check_program_types_standard_collection_methods() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(values: Array<i32>, names: List<string>, pages: Map<string, string>, text: string) -> usize {
  let a = values.len();
  let b = names.is_empty();
  let c = pages.contains_key("home");
  let d = text.len();
  if b || c {
    return a + d;
  }
  return a;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let method_types = hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::MethodCall { .. } => output.facts.expr_types.get(&id).copied(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(method_types.len(), 4);
    assert!(matches!(
        output.store.get(method_types[0]),
        Some(Type::Primitive(PrimitiveType::USize))
    ));
    assert!(matches!(
        output.store.get(method_types[1]),
        Some(Type::Primitive(PrimitiveType::Bool))
    ));
    assert!(matches!(
        output.store.get(method_types[2]),
        Some(Type::Primitive(PrimitiveType::Bool))
    ));
    assert!(matches!(
        output.store.get(method_types[3]),
        Some(Type::Primitive(PrimitiveType::USize))
    ));
}

#[test]
fn check_program_rejects_unsupported_standard_collection_method_receiver() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(value: i32) -> usize {
  return value.len();
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_types_array_list_and_empty_sequence_literals() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> i32 {
  let array = [1, 2, 3];
  let list = [1; 2; 3];
  let cons = 1 :: 2 :: [];
  let empty_array: Array<i32> = [];
  let empty_list: List<i32> = [];
  return array[0] + list[0] + cons[0];
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_uses_callable_parameter_type_for_empty_sequence_argument() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow takes_array(values: Array<i32>) -> i32 {
  return 0;
}

flow main() -> i32 {
  return takes_array([]);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_uses_later_callable_parameter_type_for_empty_sequence_argument() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow try_match_left(
  left: i32,
  sorted_edges: Array<i32>,
  pairs: Array<i32>,
  seen_rights: Array<i32>,
) -> i32 {
  return left;
}

flow main(sorted_edges: Array<i32>, pairs: Array<i32>) -> i32 {
  let result = try_match_left(1, sorted_edges, pairs, []);
  return result;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_range_literals_and_for_iteration() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> i32 {
  let range: Range<i32> = [0, 4);
  var sum: i32 = 0;
  for value in range {
    sum = sum + value;
  }
  return sum;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_slice_expressions_and_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(values: Array<i32>) -> i32 {
  let left: Slice<i32> = values[1, 3);
  let right: Slice<i32> = values(0, 2];
  return left[0] + right[0];
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.slice_facts.len(), 2);
    assert!(output.facts.index_facts.len() >= 2);
}

#[test]
fn check_program_types_range_slice_expression() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> Range<i32> {
  let range: Range<i32> = [0, 10);
  return range[2, 5);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.slice_facts.len(), 1);
}

#[test]
fn check_program_preserves_applied_standard_collection_types() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(
  deque: Deque<i32>,
  queue: Queue<string>,
  stack: Stack<i32>,
  priority: PriorityQueue<string, i32>,
  ordered_map: OrderedMap<string, i32>,
  ordered_set: OrderedSet<string>,
) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    let applied = output
        .store
        .iter()
        .filter_map(|(_, ty)| match ty {
            Type::Applied { constructor, args } => {
                let constructor = output.store.get(TypeId(constructor.0))?;
                let Type::Named(named) = constructor else {
                    return None;
                };
                Some((named.name.as_str(), args.len()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    for (name, arity) in [
        ("std.collections.Deque", 1usize),
        ("std.collections.Queue", 1),
        ("std.collections.Stack", 1),
        ("std.collections.PriorityQueue", 2),
        ("std.collections.OrderedMap", 2),
        ("std.collections.OrderedSet", 1),
    ] {
        assert!(
            applied
                .iter()
                .any(|(actual_name, actual_arity)| *actual_name == name && *actual_arity == arity),
            "missing applied type {name}/{arity}; got {applied:?}"
        );
    }
}

#[test]
fn check_program_rejects_non_index_range_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"flow main() -> unit { let bad = [0, "x"); return; }"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_rejects_list_cons_with_non_list_tail() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        "flow main() -> unit { let value = 1 :: [2, 3]; return; }",
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_rejects_uncontextualized_empty_sequence_literal() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        "flow main() -> unit { let value = []; return; }",
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
            && diagnostic
                .message
                .contains("empty sequence literal requires an expected Array[T] or List[T] type")
    }));
}

#[test]
fn check_program_types_map_set_and_empty_brace_literals() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> i32 {
  let scores = { "alice" => 10, "bob" => 8 };
  let seen = #{1, 2, 3};
  let empty_map: Map<string, i32> = {};
  let empty_set: Set<i32> = #{};
  return scores["alice"];
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_uncontextualized_empty_brace_and_set_literals() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        "flow main() -> unit { let map = {}; let set = #{}; return; }",
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
            && diagnostic
                .message
                .contains("empty brace literal requires an expected record or Map[K, V] type")
    }));
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
            && diagnostic
                .message
                .contains("empty set literal requires an expected Set[T] type")
    }));
}

#[test]
fn check_program_types_len_and_is_empty_for_string_and_map() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.collections.{is_empty, len};

flow measure(xs: List<string>, table: Map<string, i64>, text: string) -> usize {
  if is_empty(table) && !is_empty(text) {
    return len(xs) + len(table) + len(text);
  }
  return 0;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_list_index_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow first(xs: List<string>) -> string {
  return xs["zero"];
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_lambda_expression_and_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> string {
  let echo = (input: string) => input;
  return echo("ok");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let lambda_ty = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Lambda { .. } => output.facts.expr_types.get(&id).copied(),
            _ => None,
        })
        .expect("lambda expression should have a type fact");
    let Some(Type::Function(flow)) = output.store.get(lambda_ty) else {
        panic!(
            "expected lambda function type, got {:?}",
            output.store.get(lambda_ty)
        );
    };
    assert_eq!(flow.input.len(), 1);
    assert!(matches!(
        output.store.get(flow.output),
        Some(Type::Primitive(PrimitiveType::String))
    ));
}

#[test]
fn check_program_reports_untyped_lambda_parameter() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  let echo = input => input;
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::UnknownType)
    }));
}

#[test]
fn check_program_types_for_while_and_retry_statements() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(xs: List<string>, flag: bool) -> unit {
  for item in xs {
    let copy: string = item;
  }
  while flag {
    let still_bool: bool = flag;
  }
  retry {
    let value: i64 = 1;
  }
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_invalid_for_iterator_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(input: string) -> unit {
  for item in input {
    let copy = item;
  }
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_reports_while_condition_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(input: string) -> unit {
  while input {
    let copy = input;
  }
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_types_stage_composition_and_pipeline_application() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow trim(input: string) -> string {
  return input;
}

flow length(input: string) -> i64 {
  return 1;
}

flow main(input: string) -> i64 {
  let pipeline = trim | length;
  return input ~> pipeline;
}

flow network(input: string) -> string ![Network] {
  return input;
}

flow inferred(input: string) -> string {
  return input;
}

flow latent(input: string) -> string {
  let pipeline = network | inferred;
  return input ~> pipeline;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let composed = hir
        .exprs
        .iter()
        .filter_map(|(id, expr)| match expr {
            etas_hir::HirExpr::StageCompose { .. } => output.facts.expr_types.get(&id).copied(),
            _ => None,
        })
        .collect::<Vec<_>>();
    let composed_ty = composed[0];
    let Some(Type::Function(flow)) = output.store.get(composed_ty) else {
        panic!(
            "expected composed function type, got {:?}",
            output.store.get(composed_ty)
        );
    };
    assert_eq!(flow.input.len(), 1);
    assert!(matches!(
        output.store.get(flow.input[0]),
        Some(Type::Primitive(PrimitiveType::String))
    ));
    assert!(matches!(
        output.store.get(flow.output),
        Some(Type::Primitive(PrimitiveType::I64))
    ));
    let Some(Type::Function(latent_flow)) = output.store.get(composed[1]) else {
        panic!(
            "expected second composed function type, got {:?}",
            output.store.get(composed[1])
        );
    };
    assert!(
        latent_flow.effects.is_none(),
        "composition with an inferred stage must keep FlowType.effects = None"
    );
}

#[test]
fn check_program_types_agent_run_method_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer(input: Prompt) -> string ![] {
  return input;
}

flow main(input: Prompt) -> string {
  return Reviewer.run(input);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (agent_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Agent(agent) => Some((id, agent)),
            _ => None,
        })
        .expect("agent should lower");
    let agent_signature = match output.facts.item_signatures.get(&agent_id) {
        Some(etas_types::ItemSignature::Agent(signature)) => signature,
        other => panic!("expected agent signature, got {other:?}"),
    };
    let agent_effects = agent_signature
        .effects
        .as_ref()
        .expect("agent effect suffix should be typed");
    assert!(
        agent_effects.effects.is_empty(),
        "agent declarations must not publish Agentic.infer as an escaping effect"
    );
    let method_call_ty = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::MethodCall { .. } => output.facts.expr_types.get(&id).copied(),
            _ => None,
        })
        .expect("agent run method call should have a type fact");
    assert!(matches!(
        output.store.get(method_call_ty),
        Some(Type::Primitive(PrimitiveType::String))
    ));
}

#[test]
fn check_program_accepts_bodyless_agent_declaration() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer;
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (agent_id, _) = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            HirItem::Agent(agent) => Some((id, agent)),
            _ => None,
        })
        .expect("agent should lower");
    assert!(
        !output.facts.item_signatures.contains_key(&agent_id),
        "bodyless agent declaration must not materialize a callable agent signature"
    );
}

#[test]
fn check_program_rejects_bodyless_agent_run() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
agent Reviewer;

flow main(input: string) -> string {
  return Reviewer.run(input);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::UnknownField)
            || diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_types_prompt_builder_methods_and_direct_agent_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

agent Reviewer(input: string) -> string {
  return Prompt.new().system(Trusted("review")).user(Public(input));
}

flow main(input: string) -> string {
  return Reviewer(input);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_trust_wrappers_for_prompt_encoding() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

flow main(input: string) -> Prompt {
  let trusted: Trusted<string> = Trusted(input);
  let public: Public<string> = Public(input);
  let sanitized: Sanitized<string> = Sanitized(input);
  return Prompt.new().system(trusted).user(public).data(sanitized);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_dynamic_untrusted_system_prompt_content() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

flow main(input: string) -> Prompt {
  return Prompt.new().system(input);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.message.contains("Prompt.system")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_secret_prompt_encoding_by_default() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

flow main(input: string) -> Prompt {
  let secret: Secret<string> = Secret(input);
  return Prompt.new().user(secret);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.message.contains("Secret[T]")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_types_message_new_and_identity_cast() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.message.Message;

flow main(input: string) -> Option<Message<string>> {
  let message = Message.new(input);
  return message.cast<string>();
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_message_body_as_payload() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.message.Message;

flow main(input: Message<string>) -> string {
  return input.body;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_message_content_and_metadata_fields() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.message.{Message, MessageId, Participant, Role, Provenance};
import std.agent.session.SessionId;

flow content(input: Message<string>) -> string {
  return input.content;
}

flow id(input: Message<string>) -> MessageId {
  return input.id;
}

flow sender(input: Message<string>) -> Option<Participant> {
  return input.from;
}

flow role(input: Message<string>) -> Role {
  return input.role;
}

flow session(input: Message<string>) -> Option<SessionId> {
  return input.session;
}

flow provenance(input: Message<string>) -> Option<Provenance> {
  return input.provenance;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_agent_runtime_support_surfaces() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.schema.{ModelResponse, ResponseDecode};
import std.agent.message.Provenance;

flow main(
  response: ModelResponse,
  decoder: ResponseDecode<string>,
  provenance: Provenance
) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_allows_message_cast_to_checked_payload_option() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.message.Message;

flow main(input: string) -> Option<Message<i32>> {
  let message = Message.new(input);
  return message.cast<i32>();
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_session_policy_constructors_from_std_prelude() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(limit: Limit) -> unit {
  let recent: ContextPolicy = LastTurns(8);
  let summarized: ContextPolicy = SummaryPlusRecent(4);
  let retention: RetentionPolicy = Days(90);
  let compaction: CompactionPolicy = SummarizeWhen(limit);
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_session_config_continue_or_new_method() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ticket: string) -> SessionConfig {
  return SessionConfig.continue_or_new(ticket);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_session_config_record_shape() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ticket: SessionId, limit: Limit) -> SessionConfig {
  return SessionConfig {
    id = ticket,
    context = SummaryPlusRecent(4),
    retention = Days(90),
    compaction = SummarizeWhen(limit),
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_session_config_fields() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(ticket: string, limit: Limit) -> (SessionId, ContextPolicy, RetentionPolicy, CompactionPolicy) {
  let continued = SessionConfig.continue_or_new(ticket);
  let configured = SessionConfig {
    id = continued.id,
    context = SummaryPlusRecent(4),
    retention = Days(90),
    compaction = SummarizeWhen(limit),
  };
  return (continued.id, configured.context, configured.retention, configured.compaction);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_std_prelude_support_values() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> SandboxProfile {
  return DefaultCommandSandbox;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let sandbox_symbol = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "DefaultCommandSandbox")
        .expect("DefaultCommandSandbox prelude symbol should lower");
    let Some(SymbolTypeFact::Value { ty }) = output.facts.symbol_types.get(&sandbox_symbol.id)
    else {
        panic!(
            "DefaultCommandSandbox should have a value type fact: {:?}",
            output.facts.symbol_types.get(&sandbox_symbol.id)
        );
    };
    assert!(matches!(
        output.store.get(*ty),
        Some(Type::Named(named)) if named.name == "std.host.sandbox.SandboxProfile"
    ));
}

#[test]
fn check_program_accepts_agent_tools_config_with_tool_declarations() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

tool Search(input: string) -> string {
  return input;
}

@tools([Search])
agent Reviewer(input: string) -> string {
  return Prompt.new().system(Trusted("review")).user(Public(input));
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_flow_in_agent_tools_config() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.agent.prompt.Prompt;

flow Helper(input: string) -> string {
  return input;
}

@tools([Helper])
agent Reviewer(input: string) -> string {
  return Prompt.new().system(Trusted("review")).user(Public(input));
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic
                    .message
                    .contains("must reference tool declarations")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_reports_stage_composition_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow trim(input: string) -> string {
  return input;
}

flow stringify(input: i64) -> string {
  return "n";
}

flow main() -> unit {
  let bad = trim | stringify;
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_reports_non_callable_pipeline_stage() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(input: string) -> string {
  return input ~> 1;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::NonCallableCallee)
    }));
}

#[test]
fn check_program_types_tuple_and_record_patterns() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(row: { title: string, count: i64 }) -> string {
  let (title, count) = ("demo", 1);
  let { title: row_title, count: row_count } = row;
  return title;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let symbol_ty = |name: &str| {
        hir.symbols
            .iter()
            .filter(|symbol| symbol.name == name)
            .find_map(|symbol| match output.facts.symbol_types.get(&symbol.id) {
                Some(fact @ SymbolTypeFact::Local { .. }) => Some(fact),
                _ => None,
            })
    };
    assert!(matches!(
        symbol_ty("title"),
        Some(SymbolTypeFact::Local { ty, .. })
            if matches!(output.store.get(*ty), Some(Type::Primitive(PrimitiveType::String)))
    ));
    assert!(matches!(
        symbol_ty("count"),
        Some(SymbolTypeFact::Local { ty, .. })
            if matches!(output.store.get(*ty), Some(Type::Primitive(PrimitiveType::I32)))
    ));
    assert!(matches!(
        symbol_ty("row_title"),
        Some(SymbolTypeFact::Local { ty, .. })
            if matches!(output.store.get(*ty), Some(Type::Primitive(PrimitiveType::String)))
    ));
    assert!(matches!(
        symbol_ty("row_count"),
        Some(SymbolTypeFact::Local { ty, .. })
            if matches!(output.store.get(*ty), Some(Type::Primitive(PrimitiveType::I64)))
    ));
}

#[test]
fn check_program_types_literal_patterns_without_bindings() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow classify(value: i32, flag: bool) -> string {
  let first = match value {
    0 => "zero",
    _ => "many",
  };
  return match flag {
    true => first,
    false => "off",
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        hir.symbols
            .iter()
            .all(|symbol| symbol.name != "true" && symbol.name != "0"),
        "{:#?}",
        hir.symbols
    );
}

#[test]
fn check_program_ignores_never_match_arm_when_inferring_result_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow select(flag: bool) -> Result<i32, string> {
  let value = match flag {
    true => {
      return Err<i32, string>("blocked");
    },
    false => Ok<i32, string>(1),
  };
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_literal_pattern_type_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow classify(value: string) -> string {
  return match value {
    0 => "zero",
    _ => "many",
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("literal pattern does not match")
    }));
}

#[test]
fn check_program_reports_tuple_pattern_arity_mismatch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit {
  let (a, b, c) = ("demo", 1);
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
    }));
}

#[test]
fn check_program_reports_missing_record_pattern_field() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(row: { title: string }) -> unit {
  let { missing: value } = row;
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::MissingField)
    }));
}

#[test]
fn check_program_expands_resolved_record_type_aliases() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias Row = { title: string, count: i64 };

flow title(row: Row) -> string {
  return (row).title;
}

flow bind(row: Row) -> i64 {
  let { count: value } = row;
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_uses_nominal_record_representation_for_access_and_patterns() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Row = { title: string, count: i64 };

flow title(row: Row) -> string {
  return (row).title;
}

flow bind(row: Row) -> i64 {
  let { count: value } = row;
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_return_from_chained_nominal_representation() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type HeaderName = string;
type UserHeaderName = HeaderName;

flow raw(name: UserHeaderName) -> string {
  return name;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic.code, DiagnosticCode::Type(_))
                && diagnostic
                    .message
                    .contains("`UserHeaderName` is not assignable to `string`")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_nominal_as_call_argument_representation() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type HttpMethod = string;

flow needs_string(value: string) -> unit {
  return;
}

flow call(method: HttpMethod) -> unit {
  return needs_string(method);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic.code, DiagnosticCode::Type(_))
                && diagnostic.message.contains(
                    "call argument type `HttpMethod` does not match parameter type `string`",
                )
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_nominal_annotation_from_representation() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type HeaderName = string;
type UserHeaderName = HeaderName;

flow main() -> unit {
  let header: UserHeaderName = "x-user";
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic.code, DiagnosticCode::Type(_))
                && diagnostic
                    .message
                    .contains("`string` is not assignable to `UserHeaderName`")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_assigning_anonymous_record_to_nominal_record() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type User = {
  name: string,
};

flow make() -> User {
  return {
    name = "Ada",
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_returning_one_nominal_record_as_another() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type A = {
  x: i32,
};

type B = {
  x: i32,
};

flow convert(value: A) -> B {
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_accepts_explicit_nominal_record_constructor() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type User = {
  name: string,
};

flow make() -> User {
  return User {
    name = "Ada",
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_allows_nominal_record_field_projection_without_representation_assignability() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type User = {
  name: string,
};

flow read(user: User) -> string {
  return user.name;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_option_unwrap_for_result_value() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.option.unwrap;
import std.text.parse_i32;

flow parse(value: string) -> i32 {
  return unwrap(parse_i32(value));
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_types_extended_store_methods() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(store: Store<string, string>) -> unit {
  let has_paper = store.contains("paper-1");
  let paper_keys = store.keys();
  store.insert("paper-1", "draft");
  store.update("paper-1", "done");
  store.upsert("paper-2", "draft");
  store.delete("paper-1");
  store.clear();
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let expr_ty = |method_name: &str| {
        hir.exprs.iter().find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::MethodCall { method, .. } if method == method_name => {
                output.facts.expr_types.get(&id).copied()
            }
            _ => None,
        })
    };

    let contains_ty = expr_ty("contains").expect("contains should type-check");
    assert!(matches!(
        output.store.get(contains_ty),
        Some(Type::Primitive(PrimitiveType::Bool))
    ));

    let keys_ty = expr_ty("keys").expect("keys should type-check");
    assert!(matches!(
        output.store.get(keys_ty),
        Some(Type::List(inner))
            if matches!(output.store.get(*inner), Some(Type::Primitive(PrimitiveType::String)))
    ));

    for method in ["insert", "update", "upsert", "delete", "clear"] {
        let ty = expr_ty(method).unwrap_or_else(|| panic!("{method} should type-check"));
        assert!(matches!(
            output.store.get(ty),
            Some(Type::Primitive(PrimitiveType::Unit))
        ));
    }
}

#[test]
fn check_program_types_memory_selection_chains() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(store: Store<string, string>) -> unit {
  let selected = store.select("topic");
  let queried = store.query("topic");
  let scanned = store.scan();
  let related = store.related_to("topic");
  let selected_items = selected.limit(Tokens(1));
  let queried_items = queried.limit(Tokens(1));
  let scanned_items = scanned.limit(Tokens(1));
  let related_items = related.limit(Tokens(1));
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let method_ty = |method_name: &str, ordinal: usize| {
        hir.exprs
            .iter()
            .filter_map(|(id, expr)| match expr {
                etas_hir::HirExpr::MethodCall { method, .. } if method == method_name => {
                    output.facts.expr_types.get(&id).copied()
                }
                _ => None,
            })
            .nth(ordinal)
    };

    for method in ["select", "query", "scan", "related_to"] {
        let ty = method_ty(method, 0).unwrap_or_else(|| panic!("{method} should type-check"));
        assert!(matches!(
            output.store.get(ty),
            Some(Type::MemorySelection(_))
        ));
    }

    for ordinal in 0..4 {
        let ty = method_ty("limit", ordinal)
            .unwrap_or_else(|| panic!("limit #{ordinal} should type-check"));
        assert!(matches!(
            output.store.get(ty),
            Some(Type::MemorySelection(_))
        ));
    }
}

#[test]
fn check_program_types_memory_transaction_support_surfaces() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(
  transaction: MemoryTransaction,
  version: MemoryVersion,
  conflict: MemoryConflict
) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_memory_version_and_conflict_fields() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(version: MemoryVersion, conflict: MemoryConflict) -> (string, Option<MemoryVersion>, Option<MemoryVersion>, Option<JsonValue>) {
  return (version.opaque, conflict.expected, conflict.actual, conflict.current_value);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_standard_runtime_error_family() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main(
  schema: SchemaError,
  validation: ValidationError,
  timeout: ToolTimeout,
  tool_denied: ToolDenied,
  policy_denied: PolicyDenied,
  boundary: EffectBoundaryViolation,
  sandbox: SandboxViolation,
  prompt: PromptInjectionRisk,
  citation: MissingCitation,
  protocol: ProtocolViolation,
  human: HumanRejected
) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_reports_recursive_type_alias() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
alias Loop = Loop;

flow main(value: Loop) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::UnknownType)
    }));
}

#[test]
fn check_program_records_trait_signature_and_impl_fact() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec PromptEncode {
  flow encode(input: string) -> string;
}

type TlsStream = string

impl TlsStream ~ PromptEncode {
  flow encode(input: string) -> string {
    return input;
  }
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.spec_signatures.len(), 1);
    let signature = output
        .facts
        .spec_signatures
        .values()
        .next()
        .expect("spec signature should be recorded");
    assert_eq!(signature.methods.len(), 1);
    assert_eq!(signature.methods[0].name, "encode");
    assert_eq!(output.facts.spec_impls.len(), 1);
}

#[test]
fn check_program_accepts_marker_spec_impl() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec ByteStream;
type TlsStream = string
impl TlsStream ~ ByteStream;
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.spec_impls.len(), 1);
    assert_eq!(output.facts.type_spec_satisfactions.len(), 1);
}

#[test]
fn check_program_accepts_multi_marker_spec_impl() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Region;
spec Within<Root>;
spec RegionWithin<R, Root>;

type WorkspaceRoot;
type ReportsRoot;

impl ReportsRoot ~ Region + Within<ReportsRoot> + RegionWithin<ReportsRoot, WorkspaceRoot>;
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.spec_impls.len(), 3);
    assert_eq!(output.facts.type_spec_satisfactions.len(), 3);
}

#[test]
fn check_program_accepts_diamond_supertrait_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec A;
spec B ~ A;
spec C ~ A;
spec D ~ B + C;

type X;

impl X ~ D;

flow require_a<T ~ A>(value: T) -> T {
  return value;
}

flow make_x() -> X {
  return abort("x");
}

flow main() -> unit {
  let x: X = require_a<X>(make_x());
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_unrelated_diamond_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec A;
spec B;
spec C;
spec D ~ B + C;

type X;

impl X ~ D;

flow require_a<T ~ A>(value: T) -> T {
  return value;
}

flow make_x() -> X {
  return abort("x");
}

flow main() -> unit {
  let x: X = require_a<X>(make_x());
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("does not satisfy spec bound `A`")
    }));
}

#[test]
fn check_program_accepts_parameterized_supertrait_bound_substitution() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Region;
spec Within<Root>;
spec RegionWithin<R, Root> ~ Region + Within<R> + Within<Root>;

type WorkspaceRoot;
type ReportsRoot;

impl ReportsRoot ~ RegionWithin<ReportsRoot, WorkspaceRoot>;

flow require_reports_region<T ~ Region + Within<ReportsRoot>>(value: T) -> T {
  return value;
}

flow make_reports() -> ReportsRoot {
  return abort("x");
}

flow main() -> unit {
  let root: ReportsRoot = require_reports_region<ReportsRoot>(make_reports());
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_records_trace_spec_conformance_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Safe: trace = +Console.stdout_write;

flow Log(text: string) -> unit ~ Safe {
  return;
}

flow Inline(text: string) -> unit ~ (+Console.stdout_write) {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.trace_spec_conformances.len(), 2);
}

#[test]
fn check_program_accepts_flow_spec_satisfaction_with_inferred_args() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Pure<I, O> I => O ![];

flow Normalize(text: string) -> string ~ Pure {
  return text;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.callable_spec_satisfactions.len(), 1);
    assert_eq!(output.facts.trace_spec_conformances.len(), 0);
}

#[test]
fn check_program_accepts_flow_spec_satisfaction_with_effect_row_param() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
effect Console;

spec Stage<I, O, effect E> I => O ![E];

flow Print(text: string) -> string ![Console] ~ Stage {
  return text;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.callable_spec_satisfactions.len(), 1);
}

#[test]
fn check_program_rejects_type_trait_on_flow() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Marker;

flow Normalize(text: string) -> string ~ Marker {
  return text;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplTargetKind)
            && diagnostic.message.contains("cannot be used as a flow spec")
    }));
}

#[test]
fn check_program_rejects_type_trait_on_tool() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Marker;

tool Search(q: string) -> string ~ Marker {
  return q;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplTargetKind)
            && diagnostic.message.contains("cannot be used as a tool spec")
    }));
}

#[test]
fn check_program_rejects_type_trait_on_agent() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Marker;

agent Reviewer(input: string) -> string ~ Marker {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplTargetKind)
            && diagnostic
                .message
                .contains("cannot be used as an agent spec")
    }));
}

#[test]
fn check_program_accepts_protocol_declaration_conformance_target() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
protocol Review {}

flow main() -> unit ~ Review {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_non_conformance_target() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow Helper() -> unit {
  return;
}

flow main() -> unit ~ Helper {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplTargetKind)
            && diagnostic
                .message
                .contains("must resolve to a spec or protocol")
    }));
}

#[test]
fn check_program_rejects_std_non_spec_conformance_target() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow main() -> unit ~ Tokens {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplTargetKind)
                && diagnostic
                    .message
                    .contains("must resolve to a spec or protocol")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_flow_spec_as_type_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Pure<I, O> I => O ![];

flow bad<T ~ Pure>(value: T) -> T {
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("cannot be used as a type parameter bound")
    }));
}

#[test]
fn check_program_accepts_multi_spec_impl_with_one_method_spec() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Marker;
spec PromptEncode {
  flow encode(input: string) -> string;
}

type TlsStream = string

impl TlsStream ~ Marker + PromptEncode {
  flow encode(input: string) -> string {
    return input;
  }
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.spec_impls.len(), 2);
    assert!(
        output
            .facts
            .spec_impls
            .iter()
            .any(|impl_fact| impl_fact.methods.is_empty())
    );
    assert!(
        output
            .facts
            .spec_impls
            .iter()
            .any(|impl_fact| impl_fact.methods.len() == 1)
    );
}

#[test]
fn check_program_types_spec_method_selection_with_explicit_evidence() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Prompt;

spec PromptEncode {
  flow encode(input: Prompt) -> string;
}

impl Prompt ~ PromptEncode {
  flow encode(input: Prompt) -> string {
    return "encoded";
  }
}

flow encode_prompt(prompt: Prompt) -> string {
  return prompt::PromptEncode.encode();
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_spec_method_selection_without_evidence() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Prompt;

spec PromptEncode {
  flow encode(input: Prompt) -> string;
}

flow encode_prompt(prompt: Prompt) -> string {
  return prompt::PromptEncode.encode();
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("does not satisfy spec bound `PromptEncode`")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_bodyless_multi_spec_impl_with_method_spec() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec Marker;
spec PromptEncode {
  flow encode(input: string) -> string;
}

type TlsStream = string

impl TlsStream ~ Marker + PromptEncode;
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplMemberKind)
            && diagnostic.message.contains("bodyless")
            && diagnostic.message.contains("PromptEncode")
    }));
}

#[test]
fn check_program_rejects_multi_spec_impl_with_two_method_specs() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec PromptEncode {
  flow encode(input: string) -> string;
}

spec PromptDecode {
  flow decode(input: string) -> string;
}

type TlsStream = string

impl TlsStream ~ PromptEncode + PromptDecode {
  flow encode(input: string) -> string {
    return input;
  }
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplMemberKind)
            && diagnostic
                .message
                .contains("at most one method-bearing spec")
    }));
}

#[test]
fn check_program_rejects_spec_impl_missing_required_method() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec ByteStream {
  flow read(input: string) -> string;
  flow close(input: string) -> unit;
}

type TlsStream = string

impl TlsStream ~ ByteStream {
  flow read(input: string) -> string {
    return input;
  }
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplMemberKind)
            && diagnostic
                .message
                .contains("missing required method `close`")
    }));
}

#[test]
fn check_program_rejects_spec_impl_extra_method() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec ByteStream {
  flow read(input: string) -> string;
}

type TlsStream = string

impl TlsStream ~ ByteStream {
  flow read(input: string) -> string {
    return input;
  }

  flow write(input: string) -> unit {
    return;
  }
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplMemberKind)
            && diagnostic
                .message
                .contains("not declared by spec `ByteStream`")
    }));
}

#[test]
fn check_program_rejects_duplicate_spec_impl() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec ByteStream;
type TlsStream = string
impl TlsStream ~ ByteStream;
impl TlsStream ~ ByteStream;
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidImplTargetKind)
            && diagnostic
                .message
                .contains("duplicate impl of spec `ByteStream`")
    }));
}

#[test]
fn check_program_rejects_trait_name_as_value_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
spec ByteStream;

flow bad(stream: ByteStream) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("spec name cannot be used as a value type")
    }));
}

#[test]
fn check_program_rejects_std_typeclass_as_value_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.stream.ByteStream;

flow bad(stream: ByteStream) -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("spec name cannot be used as a value type")
    }));
}

#[test]
fn check_program_accepts_tls_stream_for_std_stream_trait_bound() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.stream.{ByteLimit, StreamError, Timeout, read_until_limit};
import std.tls.TlsStream;

flow read(stream: TlsStream, limit: ByteLimit, timeout: Option<Timeout>) -> bytes ![Error<StreamError>] {
  return read_until_limit(stream, limit, timeout);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_source_visible_stream_error_variants() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.stream.{StreamError, TimedOut, LimitExceeded, Host};

flow timeout_error() -> StreamError {
  return TimedOut;
}

flow limit_error() -> StreamError {
  return LimitExceeded;
}

flow qualified_limit_error() -> StreamError {
  return StreamError.LimitExceeded;
}

flow host_error(message: string) -> StreamError {
  return Host(message);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_types_source_visible_http_wire_records() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.codec.text.utf8_encode;
import std.http.codec.{HttpHeader, HttpWireRequest, HttpWireResponse, HttpWireResponseHead};

flow request() -> HttpWireRequest {
  let headers: List<HttpHeader> = [];
  return HttpWireRequest {
    method = "GET",
    target = "/",
    version = "HTTP/1.1",
    headers = headers,
    body = utf8_encode(""),
  };
}

flow response(head: HttpWireResponseHead) -> HttpWireResponse {
  return HttpWireResponse {
    head = head,
    body = utf8_encode("ok"),
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_effect_row_generic_arg_in_type_only_call() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow bad<effect E>() -> unit ![E] {
  unknown<![Console.stdout_write, E]>();
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic
                .message
                .contains("effect-row generic argument is only valid")
    }));
}

#[test]
fn check_program_accepts_effect_row_generic_arg_for_row_polymorphic_flow() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow call_with_console() -> unit ![Console.stdout_write] {
  accepts_row<![Console.stdout_write]>();
  return;
}

flow accepts_row<effect E>() -> unit ![E] {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_source_bodied_generic_tool() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
tool identity<T>(value: T) -> T {
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_imported_error_type_in_effect_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module errors;

public type HttpError = {
  kind: string,
};

module api;

import errors.HttpError;

flow raise_http_error(error: HttpError) -> never ![Error<HttpError>] {
  return perform Error<HttpError>.raise(error);
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_specializes_handler_action_selector_payload_type() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type HttpError = {
  kind: string,
};

flow recover(error: HttpError) -> string {
  return perform Error<HttpError>.raise(error) with {
    Error<HttpError>.raise(err) => {
      finish err.kind;
    }
  };
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_imported_nominal_record_alias_fields() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module types;

public type HttpUrl = {
  host: string,
};

public alias PublicHttpUrl = HttpUrl;

module api;

import types.PublicHttpUrl;

flow host(url: PublicHttpUrl) -> string {
  return url.host;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_generic_alias_record_constructor_and_projection() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
module headers;

public spec HeaderName;
public type UserHeaderName = string;
public type HeaderValue = string;
impl UserHeaderName ~ HeaderName;

public alias HeaderSpec<N ~ HeaderName> = {
  name: N,
  value: HeaderValue,
};

public alias Header = HeaderSpec<UserHeaderName>;

flow raw_header_spec(name: UserHeaderName, value: HeaderValue) -> HeaderSpec<UserHeaderName> {
  return HeaderSpec<UserHeaderName> {
    name = name,
    value = value,
  };
}

flow header_name(item: Header) -> UserHeaderName {
  let spec: HeaderSpec<UserHeaderName> = item;
  return spec.name;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_static_agent_annotations() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
tool Search(q: string) -> string;

@model("local", adapter = "omlx-openai")
@tools([Search])
@limits([Tokens(128)])
@trace(VirtualStages([Logical, Search]))
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_user_defined_trace_constructor_in_annotation() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow VirtualStages(stages: Array[string]) -> string {
  return "not std trace";
}

@trace(VirtualStages(["logical"]))
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("annotation argument must be")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_model_annotation_unknown_argument() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@model(profile = "local")
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("unsupported `@model` argument")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_model_annotation_non_string_model() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@model(42)
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("must be a string literal")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_accepts_user_defined_static_annotation_metadata() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@doc("public API")
flow main() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_test_deprecated_and_optimization_annotations() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Fusable;

@optimization([Fusable])
flow optimized() -> unit {
  return;
}

@deprecated("use optimized")
flow old_api() -> unit {
  return optimized();
}

@test
flow optimized_test() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_test_annotation_arguments() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@test("slow")
flow should_be_marker_only() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("`@test` does not accept")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_deprecated_non_string_message() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@deprecated(42)
flow old_api() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic
                    .message
                    .contains("`@deprecated` message must be a string literal")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_optimization_non_path_marker() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@optimization([42])
flow optimized() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic
                    .message
                    .contains("`@optimization` entries must be static path markers")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_runtime_annotation_argument() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow provider() -> string {
  return "omlx-openai";
}

@model(adapter = provider())
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("static literal")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_user_defined_limit_constructor_in_annotation() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow Tokens(count: usize) -> usize {
  return count;
}

@limits([Tokens(128)])
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("annotation argument must be")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_unresolved_annotation_path_argument() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@deprecated(Missing.AnnotationValue)
flow old_api() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("annotation argument must be")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_tools_annotation_on_flow() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
tool Search(q: string) -> string;

@tools([Search])
flow main() -> unit {
  return;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic.message.contains("cannot be applied")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_non_tool_tools_annotation_entry() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow NotATool(q: string) -> string {
  return q;
}

@tools([NotATool])
agent Reviewer(input: string) -> string {
  return abort("not implemented");
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic
                    .message
                    .contains("must reference tool declarations")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_accepts_derivable_std_annotation_entries() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
@derive([Schema, PromptEncode])
type Review = {
  summary: string,
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_accepts_schema_and_response_decode_as_specs_and_descriptor_types() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
type Review = {
  summary: string,
}

type DecodeContext = {
  schema: Schema<Review>,
  decoder: ResponseDecode<Review>,
}

flow accepts_schema_bound<T ~ Schema + ResponseDecode>(value: T) -> T {
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn check_program_rejects_schema_spec_bound_with_descriptor_type_arg() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow invalid<T ~ Schema<string>>(value: T) -> T {
  return value;
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::ArityMismatch)
                && diagnostic.message.contains("spec `Schema` expects 0")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn check_program_rejects_non_derivable_annotation_entry() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
flow NotDerivable() -> unit {
  return;
}

@derive([NotDerivable])
type Review = {
  summary: string,
}
"#,
    ));
    let hir = lower_program(&parsed.value);
    let output = check_program(&hir);

    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::InvalidAnnotation)
                && diagnostic
                    .message
                    .contains("compiler-derivable std capabilities")
        }),
        "{:?}",
        output.diagnostics
    );
}
