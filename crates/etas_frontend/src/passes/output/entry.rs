use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span, TextSize};
use etas_hir::{HirItem, HirItemId, HirModuleId, HirTreeView};
use etas_types::{FlowSignature, ItemSignature, PrimitiveType, Type, TypeStore};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{EntryPolicy, ModulePath, ProjectContext, ProjectEntryFact, ProjectEntryResolution};

use crate::passes::artifacts::{
    HIR_OUTPUT, MODULE_INDEX, PROJECT_ENTRY, TYPE_OUTPUT, global_with_diagnostics,
};

pub struct ResolveEntryItemPass;

impl Pass<ProjectContext> for ResolveEntryItemPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ResolveEntryItemPass", PassKind::Analysis)
            .requires(ArtifactSet::from([MODULE_INDEX, HIR_OUTPUT]))
            .produces(global_with_diagnostics([PROJECT_ENTRY]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let requested = context.input.entry.clone();
        let (resolved, diagnostics) = resolve_entry(context, &requested.module, &requested.flow);
        context.diagnostics.extend(diagnostics);
        context.entry = Some(ProjectEntryFact {
            requested,
            resolved,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(PROJECT_ENTRY))
    }
}

pub struct ValidateEntryContractPass;

impl Pass<ProjectContext> for ValidateEntryContractPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateEntryContractPass", PassKind::Verify)
            .requires(ArtifactSet::from([PROJECT_ENTRY, TYPE_OUTPUT]))
            .produces(global_with_diagnostics([]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(entry) = context.entry.as_ref() else {
            return PassResult::changed(PreservedArtifacts::All, global_with_diagnostics([]));
        };
        let Some(resolution) = entry.resolved.as_ref() else {
            return PassResult::changed(PreservedArtifacts::All, global_with_diagnostics([]));
        };

        let flow_name = entry.requested.flow.clone();
        if !entry_has_flow_signature(context, resolution.item) {
            context.diagnostics.push(project_diagnostic(
                entry_diagnostic_span(context),
                &format!("entry flow `{flow_name}` does not have a flow signature"),
            ));
        } else if context.input.options.entry_policy == EntryPolicy::Runnable
            && !entry_has_runnable_contract(context, resolution.item)
        {
            context.diagnostics.push(project_diagnostic(
                entry_diagnostic_span(context),
                &format!(
                    "entry flow `{flow_name}` must resolve to `flow {flow_name}(args: Array[string]) -> i32`"
                ),
            ));
        }

        PassResult::changed(PreservedArtifacts::All, global_with_diagnostics([]))
    }
}

fn resolve_entry(
    context: &ProjectContext,
    requested_module: &Option<ModulePath>,
    flow_name: &str,
) -> (Option<ProjectEntryResolution>, Vec<Diagnostic>) {
    let hir = context.hir.as_ref().expect("HIR output should exist");
    let hir_view = hir.view();
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();

    if let Some(module_path) = requested_module {
        let Some(module) = hir_module_for_path(&hir_view, module_path) else {
            diagnostics.push(project_diagnostic(
                entry_diagnostic_span(context),
                &format!(
                    "entry module `{}` was not found",
                    module_path_text(module_path)
                ),
            ));
            return (None, diagnostics);
        };
        candidates.extend(flow_items_in_module(&hir_view, module, flow_name));
    } else {
        for module in hir_view.modules() {
            candidates.extend(flow_items_in_module(&hir_view, module.id(), flow_name));
        }
    }

    match candidates.as_slice() {
        [] => {
            if context.input.options.entry_policy == EntryPolicy::Optional {
                return (None, diagnostics);
            }
            diagnostics.push(project_diagnostic(
                entry_diagnostic_span(context),
                &format!("entry flow `{flow_name}` was not found"),
            ));
            (None, diagnostics)
        }
        [resolution] => (Some(resolution.clone()), diagnostics),
        _ => {
            diagnostics.push(project_diagnostic(
                entry_diagnostic_span(context),
                &format!("entry flow `{flow_name}` is ambiguous; specify an entry module"),
            ));
            (None, diagnostics)
        }
    }
}

fn hir_module_for_path(hir_view: &HirTreeView<'_>, path: &ModulePath) -> Option<HirModuleId> {
    hir_view.modules().into_iter().find_map(|module| {
        module.data().name.as_ref().and_then(|name| {
            name.segments
                .iter()
                .map(|segment| segment.name.as_str())
                .eq(path.segments.iter().map(String::as_str))
                .then_some(module.id())
        })
    })
}

fn flow_items_in_module(
    hir_view: &HirTreeView<'_>,
    module: HirModuleId,
    flow_name: &str,
) -> Vec<ProjectEntryResolution> {
    let Some(module) = hir_view.module(module) else {
        return Vec::new();
    };
    module
        .items()
        .into_iter()
        .filter_map(|item| {
            let HirItem::Flow(flow) = item.data() else {
                return None;
            };
            let symbol = hir_view.program().symbols.get(flow.symbol)?;
            (symbol.name == flow_name).then_some(ProjectEntryResolution {
                module: module.id(),
                item: item.id(),
            })
        })
        .collect()
}

fn entry_has_flow_signature(context: &ProjectContext, item: HirItemId) -> bool {
    context
        .types
        .as_ref()
        .and_then(|types| types.facts.item_signatures.get(&item))
        .is_some_and(|signature| matches!(signature, ItemSignature::Flow(_)))
}

fn entry_has_runnable_contract(context: &ProjectContext, item: HirItemId) -> bool {
    let Some(types) = context.types.as_ref() else {
        return false;
    };
    let Some(ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item) else {
        return false;
    };
    runnable_flow_signature(signature, &types.store)
}

fn runnable_flow_signature(signature: &FlowSignature, store: &TypeStore) -> bool {
    signature.params.len() == 1
        && is_array_string(signature.params[0], store)
        && is_primitive(signature.output, PrimitiveType::I32, store)
}

fn is_array_string(ty: etas_types::TypeId, store: &TypeStore) -> bool {
    let Some(Type::Array(inner)) = store.get(ty) else {
        return false;
    };
    is_primitive(*inner, PrimitiveType::String, store)
}

fn is_primitive(ty: etas_types::TypeId, expected: PrimitiveType, store: &TypeStore) -> bool {
    matches!(store.get(ty), Some(Type::Primitive(actual)) if *actual == expected)
}

fn entry_diagnostic_span(context: &ProjectContext) -> Span {
    context
        .sources
        .as_ref()
        .and_then(|sources| sources.files.first())
        .map_or_else(
            || Span::empty(etas_core::SourceId(0), TextSize::ZERO),
            |source| Span::empty(source.id, TextSize::ZERO),
        )
}

fn project_diagnostic(span: Span, message: &str) -> Diagnostic {
    Diagnostic::analysis(AnalysisDiagnosticCode::InvalidEntry, span, message)
}

fn module_path_text(path: &ModulePath) -> String {
    path.segments.join(".")
}
