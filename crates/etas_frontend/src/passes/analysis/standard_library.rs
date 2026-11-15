use std::collections::HashMap;
use std::sync::Arc;

use etas_core::SourceId;
use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_std::{
    StdImplementationDeterminism, StdImplementationRef, StdIntrinsicId, StdLibrarySourceId,
    StdSymbolId,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    ADMITTED_STD_LIBRARY_SOURCES, CHECKED_STD_LIBRARY_IMPLEMENTATIONS, EFFECT_OUTPUT, HIR_OUTPUT,
    TYPE_OUTPUT,
};

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct CheckedStandardLibraryImplementation {
    pub declaration: StdSymbolId,
    pub source: StdLibrarySourceId,
    pub item: HirItemId,
    pub symbol: SymbolId,
    pub optimized_intrinsic: Option<StdIntrinsicId>,
    pub signature: etas_types::CallableSignature,
    pub effects: etas_effects::EffectRow,
    pub requested_actions: etas_effects::EffectRow,
    pub determinism: etas_effects::Determinism,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct CheckedStandardLibraryImplementations {
    pub implementations: Vec<CheckedStandardLibraryImplementation>,
}

pub struct CheckStandardLibraryImplementationsPass;

impl Pass<ProjectContext> for CheckStandardLibraryImplementationsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new(
            "CheckStandardLibraryImplementationsPass",
            PassKind::Analysis,
        )
        .requires(ArtifactSet::from([
            ADMITTED_STD_LIBRARY_SOURCES,
            HIR_OUTPUT,
            TYPE_OUTPUT,
            EFFECT_OUTPUT,
        ]))
        .produces(ArtifactSet::one(CHECKED_STD_LIBRARY_IMPLEMENTATIONS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(admitted), Some(hir), Some(types), Some(effects), Some(effect_artifacts)) = (
            context.admitted_std_library_sources.as_ref(),
            context.hir.as_ref(),
            context.types.as_ref(),
            context.effects.as_ref(),
            context.effect_pipeline_artifacts.as_ref(),
        ) else {
            return PassResult::failed(
                "standard library implementation checking requires finalized type and effect facts",
            );
        };

        context
            .diagnostics
            .extend(native_enum_reference_diagnostics(
                &hir.hir,
                &context.std_registry,
                types,
            ));

        context
            .diagnostics
            .extend(native_opaque_reference_diagnostics(
                &hir.hir,
                &context.std_registry,
            ));

        context
            .diagnostics
            .extend(native_record_reference_diagnostics(
                &hir.hir,
                &context.std_registry,
                types,
            ));

        context
            .diagnostics
            .extend(native_projection_reference_diagnostics(
                &hir.hir,
                &context.std_registry,
                types,
            ));

        for (item, target, span) in
            super::super::hir::native_binding::declarations(&hir.hir, &context.std_registry)
        {
            let Some(etas_types::ItemSignature::Flow(source)) =
                types.facts.item_signatures.get(&item)
            else {
                continue;
            };
            let etas_hir::HirItem::Flow(flow) = &hir.hir.items[item] else {
                continue;
            };
            let Some(alias) = hir
                .hir
                .scopes
                .lookup_local(flow.scope, &format!("$std_binding_{}", flow.symbol.0))
            else {
                continue;
            };
            let Some(etas_types::SymbolTypeFact::Flow {
                signature: expected,
            }) = types.facts.symbol_types.get(&alias)
            else {
                return PassResult::failed("native binding has no checked primitive signature");
            };
            if !native_signatures_match(source, expected, &types.store, &types.facts)
                .unwrap_or(false)
            {
                context.diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::TypeMismatch, span,
                    format!("std native binding ABI differs from `{}`; match its generic bounds, parameters, result and public effects", context.std_registry.symbol(target).unwrap().qualified_path.join(".")),
                ));
            }
        }

        let admitted_by_source = admitted
            .sources
            .iter()
            .map(|source| (source.source, source.library_source))
            .collect::<HashMap<SourceId, StdLibrarySourceId>>();
        let mut source_items =
            HashMap::<(StdLibrarySourceId, String), Vec<(HirItemId, SymbolId)>>::new();
        for (item, declaration) in hir.hir.items.iter() {
            let HirItem::Flow(flow) = declaration else {
                continue;
            };
            let Some(source) = admitted_by_source.get(&flow.span.source).copied() else {
                continue;
            };
            let Some(symbol) = hir.hir.symbols.get(flow.symbol) else {
                return PassResult::failed(
                    "standard library source flow is missing its checked symbol",
                );
            };
            source_items
                .entry((source, symbol.name.clone()))
                .or_default()
                .push((item, flow.symbol));
        }

        let mut checked = CheckedStandardLibraryImplementations::default();
        for declaration in context.std_registry.symbols() {
            let Some(StdImplementationRef::Source {
                target,
                optimized_intrinsic,
            }) = declaration.implementation
            else {
                continue;
            };
            if !admitted
                .sources
                .iter()
                .any(|source| source.library_source == target.source)
            {
                continue;
            }
            let Some(items) = source_items.get(&(target.source, target.item.to_owned())) else {
                return PassResult::failed(format!(
                    "standard declaration `{}` has no checked source implementation",
                    declaration.qualified_path.join(".")
                ));
            };
            let [(item, symbol)] = items.as_slice() else {
                return PassResult::failed(format!(
                    "standard declaration `{}` has multiple checked source implementations",
                    declaration.qualified_path.join(".")
                ));
            };
            let Some(public_signature) = types.facts.std_callable_signatures.get(&declaration.id)
            else {
                return PassResult::failed(format!(
                    "standard declaration `{}` has no checked public signature",
                    declaration.qualified_path.join(".")
                ));
            };
            let Some(etas_types::ItemSignature::Flow(source_signature)) =
                types.facts.item_signatures.get(item)
            else {
                return PassResult::failed(format!(
                    "standard source implementation `{}` has no checked flow signature",
                    target.item
                ));
            };
            if !signatures_match(source_signature, public_signature) {
                return PassResult::failed(format!(
                    "standard source implementation `{}` does not match declaration `{}`: source={source_signature:?}, declaration={public_signature:?}",
                    target.item,
                    declaration.qualified_path.join(".")
                ));
            }
            let Some(summary) = effects.facts.item_effects.get(item) else {
                return PassResult::failed(format!(
                    "standard source implementation `{}` has no solved effect summary",
                    target.item
                ));
            };
            let expected_effects = match public_signature.effects.as_ref() {
                Some(row) => etas_effects::validate::resolve_checked_type_row(
                    &types.store,
                    &effect_artifacts.registry,
                    row,
                ),
                None => Ok(etas_effects::EffectRow::empty()),
            };
            let expected_effects = match expected_effects {
                Ok(row) => row,
                Err(reason) => return PassResult::failed(reason),
            };
            let expected_requests = match public_signature.requested_actions.as_ref() {
                Some(row) => etas_effects::validate::resolve_checked_type_row(
                    &types.store,
                    &effect_artifacts.registry,
                    row,
                ),
                None => Ok(etas_effects::EffectRow::empty()),
            };
            let expected_requests = match expected_requests {
                Ok(row) => row,
                Err(reason) => return PassResult::failed(reason),
            };
            let expected_determinism = match target.determinism {
                StdImplementationDeterminism::Deterministic => {
                    etas_effects::Determinism::Deterministic
                }
                StdImplementationDeterminism::RuntimeMediated => {
                    etas_effects::Determinism::RuntimeMediated
                }
                StdImplementationDeterminism::NonDeterministic => {
                    etas_effects::Determinism::NonDeterministic
                }
            };
            if !row_is_allowed(&summary.escaping_effects, &expected_effects)
                || !row_is_allowed(&summary.requested_actions, &expected_requests)
                || summary.determinism != expected_determinism
                || matches!(
                    summary.support,
                    etas_effects::InterpreterSupport::Rejected(_)
                )
            {
                return PassResult::failed(format!(
                    "standard source implementation `{}` has behavior incompatible with declaration `{}`: source effects={:?}, source requests={:?}, source determinism={:?}; declaration effects={expected_effects:?}, declaration requests={expected_requests:?}, declaration determinism={expected_determinism:?}",
                    target.item,
                    declaration.qualified_path.join("."),
                    summary.escaping_effects,
                    summary.requested_actions,
                    summary.determinism,
                ));
            }
            checked
                .implementations
                .push(CheckedStandardLibraryImplementation {
                    declaration: declaration.id,
                    source: target.source,
                    item: *item,
                    symbol: *symbol,
                    optimized_intrinsic,
                    signature: source_signature.clone(),
                    effects: summary.escaping_effects.clone(),
                    requested_actions: summary.requested_actions.clone(),
                    determinism: summary.determinism,
                });
        }
        checked
            .implementations
            .sort_by_key(|implementation| implementation.declaration);
        context.checked_std_library_implementations = Some(Arc::new(checked));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CHECKED_STD_LIBRARY_IMPLEMENTATIONS),
        )
    }
}

fn native_enum_reference_diagnostics(
    hir: &etas_hir::HirProgram,
    registry: &etas_std::StdRegistry,
    types: &etas_types::TypeOutput,
) -> Vec<etas_core::Diagnostic> {
    let mut diagnostics = Vec::new();
    for reference in registry.native_enum_references() {
        let (name, module) = reference.path.split_last().unwrap();
        let source = hir.items.iter().find_map(|(_, item)| {
            let (symbol, span) = match item {
                HirItem::Enum(declaration) => (declaration.symbol, declaration.span),
                HirItem::Type(declaration) => (declaration.symbol, declaration.span),
                _ => return None,
            };
            let symbol = hir.symbols.get(symbol).unwrap();
            let owner = hir.modules_arena.get(symbol.defining_module).unwrap();
            (symbol.name == *name
                && owner.name.as_ref().is_some_and(|path| {
                    path.segments
                        .iter()
                        .map(|segment| &segment.name)
                        .eq(module.iter())
                }))
            .then_some((item, span))
        });
        let Some((source, span)) = source else {
            // Partial compiler library images may leave ABI imports unresolved.
            // Publishing a complete std provider requires the checked enum.
            continue;
        };
        let error = |span, reason: String| {
            etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::TypeMismatch,
                span,
                reason,
            )
        };
        let HirItem::Enum(declaration) = source else {
            diagnostics.push(error(
                span,
                format!(
                    "native ABI references `{}` as an enum",
                    reference.path.join(".")
                ),
            ));
            continue;
        };
        if !declaration.type_params.is_empty() {
            diagnostics.push(error(
                span,
                "native enum ABI reference requires no type parameters".into(),
            ));
        }
        let layout =
            types
                .facts
                .symbol_types
                .get(&declaration.symbol)
                .and_then(|fact| match fact {
                    etas_types::SymbolTypeFact::Type { constructor } => types
                        .facts
                        .enum_layouts
                        .get(&etas_types::TypeId(constructor.0)),
                    _ => None,
                });
        for (required, fields) in reference.required_variants() {
            let variant = declaration
                .variants
                .iter()
                .find(|variant| hir.symbols.get(variant.symbol).unwrap().name == required);
            let matches = layout
                .and_then(|layout| {
                    layout
                        .variants
                        .iter()
                        .find(|variant| variant.name == required)
                })
                .is_some_and(|variant| {
                    variant.fields.len() == fields.len()
                        && variant.fields.iter().zip(fields).all(|(actual, expected)| {
                            native_primitive_matches(
                                &types.store,
                                *actual,
                                &etas_std::StdType::Primitive(*expected),
                            )
                        })
                });
            if !matches {
                let reason = if fields.is_empty() {
                    format!("native enum ABI requires unit variant `{required}`")
                } else {
                    format!(
                        "native enum ABI requires variant `{required}` with checked payload {fields:?}"
                    )
                };
                diagnostics.push(error(variant.map_or(span, |variant| variant.span), reason));
            }
        }
    }
    diagnostics
}

fn row_is_allowed(actual: &etas_effects::EffectRow, declared: &etas_effects::EffectRow) -> bool {
    actual.open == declared.open
        && actual
            .effects
            .iter()
            .all(|effect| declared.effects.contains(effect))
}

fn native_signatures_match(
    source: &etas_types::CallableSignature,
    native: &etas_types::CallableSignature,
    store: &etas_types::TypeStore,
    facts: &etas_types::TypeFacts,
) -> Result<bool, etas_types::TypeSubstitutionError> {
    use etas_types::{CallableGenericParamKind as Kind, EffectRowRef, TypeSubstitutionEngine};
    if source.generic_params.len() != native.generic_params.len()
        || source
            .generic_params
            .iter()
            .zip(&native.generic_params)
            .any(|(left, right)| left.kind != right.kind)
    {
        return Ok(false);
    }
    let pairs = native
        .generic_params
        .iter()
        .zip(&source.generic_params)
        .collect::<Vec<_>>();
    let parameters = pairs
        .iter()
        .filter(|(from, _)| from.kind == Kind::Type)
        .map(|(from, to)| (from.subject, to.subject))
        .collect();
    let named = pairs
        .iter()
        .filter(|(from, _)| from.kind == Kind::Type)
        .map(|(from, to)| (from.name.clone(), to.subject))
        .collect();
    let rows = pairs
        .iter()
        .filter(|(from, _)| from.kind == Kind::Effect)
        .map(|(from, to)| {
            (
                from.name.clone(),
                EffectRowRef {
                    effects: vec![],
                    tail: Some(to.name.clone()),
                },
            )
        })
        .collect();
    let mut substitution = TypeSubstitutionEngine::existing(store, &named)
        .with_type_parameters(&parameters)
        .with_effect_arguments(&rows);
    for (from, to) in pairs {
        let mut bounds = from.bounds.clone();
        for bound in &mut bounds {
            bound.args = bound
                .args
                .iter()
                .map(|ty| substitution.substitute(*ty))
                .collect::<Result<_, _>>()?;
        }
        let mut source_bounds = to.bounds.clone();
        for bound in &mut source_bounds {
            if let etas_types::CheckedSpecRef::Source(symbol) = &bound.spec
                && let Some(path) = facts.std_spec_aliases.get(symbol)
            {
                bound.spec = etas_types::CheckedSpecRef::Std(path.clone());
            }
        }
        if bounds != source_bounds {
            return Ok(false);
        }
    }
    let params = native
        .params
        .iter()
        .map(|ty| substitution.substitute(*ty))
        .collect::<Result<Vec<_>, _>>()?;
    let output = substitution.substitute(native.output)?;
    let empty = EffectRowRef {
        effects: vec![],
        tail: None,
    };
    let effects = substitution
        .substitute_effect_row(native.effects.clone().unwrap_or_else(|| empty.clone()))?;
    // Requested actions are derived by the effect pipeline from the bound call.
    Ok(params == source.params
        && output == source.output
        && &effects == source.effects.as_ref().unwrap_or(&empty))
}

fn signatures_match(
    source: &etas_types::CallableSignature,
    declaration: &etas_types::CallableSignature,
) -> bool {
    let empty = etas_types::EffectRowRef {
        effects: Vec::new(),
        tail: None,
    };
    source.generic_params == declaration.generic_params
        && source.params == declaration.params
        && source.output == declaration.output
        && source.effects.as_ref().unwrap_or(&empty)
            == declaration.effects.as_ref().unwrap_or(&empty)
        && source.requested_actions.as_ref().unwrap_or(&empty)
            == declaration.requested_actions.as_ref().unwrap_or(&empty)
}

#[cfg(test)]
mod tests {
    use etas_std::{StdImplementationRef, StdLibrarySourceId};
    use etas_utils::{PassControl, PassManager};

    use crate::{ProjectContext, ProjectInput, SourceInput, pipeline::project_pipeline};

    #[test]
    fn source_backed_text_implementation_is_admitted_and_checked() {
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app;
import std.text.concat;
flow main() -> string { return concat(["a", "b"]); }
"#,
        )));
        let plan = crate::optimization::OptimizationPlan::from_options(&context.input.options);
        let mut pipeline = project_pipeline(&plan);
        let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);

        assert_eq!(run.control, PassControl::Continue);
        assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
        assert!(
            context
                .admitted_std_library_sources
                .as_ref()
                .unwrap()
                .sources
                .iter()
                .any(|source| source.library_source == StdLibrarySourceId::TextAssembly)
        );
        let declaration = context
            .std_registry
            .lookup_qualified(&["std", "text", "concat"])
            .unwrap();
        let checked = context
            .checked_std_library_implementations
            .as_ref()
            .unwrap()
            .implementations
            .iter()
            .find(|implementation| implementation.declaration == declaration.id)
            .unwrap();
        let Some(StdImplementationRef::Source {
            optimized_intrinsic,
            ..
        }) = declaration.implementation
        else {
            panic!("source implementation");
        };
        assert_eq!(checked.optimized_intrinsic, optimized_intrinsic);
    }
}

fn native_opaque_reference_diagnostics(
    hir: &etas_hir::HirProgram,
    registry: &etas_std::StdRegistry,
) -> Vec<etas_core::Diagnostic> {
    let mut diagnostics = Vec::new();
    for reference in registry.native_opaque_references() {
        let (name, module) = reference.path.split_last().unwrap();
        for (_, item) in hir.items.iter() {
            let Some(symbol) = item.symbol().and_then(|symbol| hir.symbols.get(symbol)) else {
                continue;
            };
            let owner = &hir.modules_arena[symbol.defining_module];
            if symbol.name != *name
                || !owner.name.as_ref().is_some_and(|path| {
                    path.segments
                        .iter()
                        .map(|part| &part.name)
                        .eq(module.iter())
                })
            {
                continue;
            }
            if !matches!(item, HirItem::Type(declaration) if declaration.type_params.is_empty() && matches!(declaration.body, etas_hir::HirTypeDeclBody::Bodyless))
            {
                diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::TypeMismatch,
                    symbol.definition_span,
                    format!(
                        "native ABI references `{}` as an opaque type without generic parameters",
                        reference.path.join(".")
                    ),
                ));
            }
        }
    }
    diagnostics
}

fn native_primitive_matches(
    store: &etas_types::TypeStore,
    actual: etas_types::TypeId,
    expected: &etas_std::StdType,
) -> bool {
    matches!(store.get(actual), Some(etas_types::Type::Primitive(primitive))
        if etas_std::StdType::parse(primitive.source_name()) == *expected)
}

fn native_record_reference_diagnostics(
    hir: &etas_hir::HirProgram,
    registry: &etas_std::StdRegistry,
    types: &etas_types::TypeOutput,
) -> Vec<etas_core::Diagnostic> {
    let mut diagnostics = Vec::new();
    for reference in registry.native_record_references() {
        let (name, module) = reference.path.split_last().unwrap();
        for (_, item) in hir.items.iter() {
            let Some(symbol) = item.symbol().and_then(|symbol| hir.symbols.get(symbol)) else {
                continue;
            };
            if symbol.name != *name
                || !hir.modules_arena[symbol.defining_module]
                    .name
                    .as_ref()
                    .is_some_and(|path| {
                        path.segments
                            .iter()
                            .map(|part| &part.name)
                            .eq(module.iter())
                    })
            {
                continue;
            }
            let representation = match types.facts.symbol_types.get(&symbol.id) {
                Some(etas_types::SymbolTypeFact::NominalType { representation, .. }) => {
                    *representation
                }
                _ => None,
            };
            let valid = matches!(item, HirItem::Type(declaration) if declaration.type_params.is_empty())
                && representation.and_then(|ty| types.store.get(ty)).is_some_and(|ty| {
                    matches!(ty, etas_types::Type::Record(record) if record.fields.len() == reference.fields.len()
                        && reference.fields.iter().all(|field| record.fields.iter().find(|actual| actual.name == field.name)
                            .is_some_and(|actual| native_record_field_matches(hir, types, actual.ty, &field.ty))))
                });
            if !valid {
                diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::TypeMismatch,
                    symbol.definition_span,
                    format!(
                        "native record ABI requires `{}` with its checked host payload fields",
                        reference.path.join(".")
                    ),
                ));
            }
        }
    }
    diagnostics
}

fn native_record_field_matches(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    actual: etas_types::TypeId,
    expected: &etas_std::StdType,
) -> bool {
    use etas_std::StdType;
    use etas_types::{SymbolTypeFact, Type, TypeId};
    match (expected, types.store.get(actual)) {
        (StdType::Primitive(_), _) => native_primitive_matches(&types.store, actual, expected),
        (StdType::Array(expected), Some(Type::Array(actual)))
        | (StdType::Option(expected), Some(Type::Option(actual))) => {
            native_record_field_matches(hir, types, *actual, expected)
        }
        (StdType::Named(expected), _) => {
            let expected = expected.split('.').collect::<Vec<_>>();
            // Match the checked declaration's TypeId, rather than accepting a
            // nominal with a similar name or record representation.
            hir.symbols.iter().any(|symbol| {
                let canonical = match &symbol.def {
                    etas_hir::SymbolDef::ImportAlias { path, .. } => {
                        path.iter().map(String::as_str).eq(expected.iter().copied())
                    }
                    _ => hir.modules_arena[symbol.defining_module]
                        .name
                        .as_ref()
                        .is_some_and(|module| {
                            module
                                .segments
                                .iter()
                                .map(|part| part.name.as_str())
                                .chain(std::iter::once(symbol.name.as_str()))
                                .eq(expected.iter().copied())
                        }),
                };
                canonical
                    && match types.facts.symbol_types.get(&symbol.id) {
                        Some(
                            SymbolTypeFact::NominalType { constructor, .. }
                            | SymbolTypeFact::Type { constructor },
                        ) => actual == TypeId(constructor.0),
                        Some(SymbolTypeFact::TypeAlias { target, params }) if params.is_empty() => {
                            actual == *target
                        }
                        _ => false,
                    }
            })
        }
        _ => false,
    }
}

fn native_projection_reference_diagnostics(
    hir: &etas_hir::HirProgram,
    registry: &etas_std::StdRegistry,
    types: &etas_types::TypeOutput,
) -> Vec<etas_core::Diagnostic> {
    let mut diagnostics = Vec::new();
    for reference in registry.native_projection_references() {
        let (name, module) = reference.path.split_last().unwrap();
        for (_, item) in hir.items.iter() {
            let Some(symbol) = item.symbol().and_then(|symbol| hir.symbols.get(symbol)) else {
                continue;
            };
            if symbol.name != *name
                || !hir.modules_arena[symbol.defining_module]
                    .name
                    .as_ref()
                    .is_some_and(|path| {
                        path.segments
                            .iter()
                            .map(|part| &part.name)
                            .eq(module.iter())
                    })
            {
                continue;
            }
            let fields = match types.facts.symbol_types.get(&symbol.id) {
                Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) => types
                    .facts
                    .known_std_types
                    .projected_fields
                    .get(&etas_types::TypeId(constructor.0)),
                _ => None,
            };
            let valid = fields.is_some_and(|fields| {
                fields.len() == reference.fields.len()
                    && reference.fields.iter().all(|field| {
                        fields
                            .iter()
                            .find(|actual| actual.name == field.name)
                            .is_some_and(|actual| {
                                native_record_field_matches(hir, types, actual.ty, &field.ty)
                            })
                    })
            });
            if !valid {
                diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::TypeMismatch,
                    symbol.definition_span,
                    format!(
                        "native getter ABI requires `{}` with its checked projected fields",
                        reference.path.join(".")
                    ),
                ));
            }
        }
    }
    diagnostics
}
