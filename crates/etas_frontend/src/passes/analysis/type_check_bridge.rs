use std::collections::HashMap;

use etas_core::{Diagnostic, TypeDiagnosticCode};

use crate::{
    ExternalPackageId, ImportTarget, ModuleId, ModulePath, ProjectContext,
    ProjectExternalActionArgKindInput, ProjectExternalActionSignatureInput,
    ProjectExternalAgentSignatureInput, ProjectExternalEffectArgInput,
    ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalFlowSignatureInput, ProjectExternalNamedSignatureInput,
    ProjectExternalPublicMetadataInput, ProjectExternalRecordFieldInput,
    ProjectExternalSpecKindInput, ProjectExternalToolSignatureInput, ProjectExternalTypeInput,
    ResolvedModuleTarget, ResolvedWildcardImport,
};

pub(super) fn build_signature_pipeline_input<'a>(
    context: &ProjectContext,
    program: &'a etas_hir::HirProgram,
) -> (etas_types::SignaturePipelineInput<'a>, Vec<Diagnostic>) {
    let Some(hir) = context.hir.as_ref().map(|output| &output.hir) else {
        return (etas_types::SignaturePipelineInput::new(program), Vec::new());
    };
    let mut diagnostics = Vec::new();
    let input = etas_types::SignaturePipelineInput {
        program,
        std_registry: context.std_registry.clone(),
        source: source_signature_input(context),
        std: std_signature_input(context, hir),
        external: external_signature_input(context, hir, &mut diagnostics),
    };
    (input, diagnostics)
}

fn external_signature_input(
    context: &ProjectContext,
    hir: &etas_hir::HirProgram,
    diagnostics: &mut Vec<Diagnostic>,
) -> etas_types::ExternalSignatureInput {
    let mut input = etas_types::ExternalSignatureInput {
        metadata: context
            .input
            .environment
            .external_public_metadata
            .iter()
            .map(convert_external_metadata)
            .collect(),
        symbol_bindings: Vec::new(),
        action_bindings: Vec::new(),
    };

    if let Some(resolved_imports) = context.resolved_imports.as_ref() {
        for resolved in &resolved_imports.imports {
            let ImportTarget::ExternalItem {
                package: Some(package),
                module_path,
                name,
                ..
            } = &resolved.target
            else {
                continue;
            };
            let Some(binding_symbol) = hir_module_for_source_module(context, resolved.from)
                .and_then(|hir_module| hir.modules_arena.get(hir_module))
                .and_then(|module| {
                    import_binding_symbol_for_import(
                        module,
                        resolved.import.source,
                        resolved.span,
                        &resolved.local_name,
                    )
                })
            else {
                continue;
            };
            input
                .symbol_bindings
                .push(etas_types::ExternalSymbolBindingInput {
                    symbol: binding_symbol,
                    package: external_package_key(*package),
                    path: external_item_path(module_path, name),
                    span: resolved.span,
                });
        }

        let wildcard_counts = wildcard_import_name_counts(&resolved_imports.wildcard_imports);
        for wildcard in &resolved_imports.wildcard_imports {
            let ResolvedModuleTarget::External {
                package: Some(package),
                path: module_path,
                ..
            } = &wildcard.target_module
            else {
                continue;
            };
            for name in &wildcard.exported_names {
                if wildcard_counts
                    .get(&(wildcard.from, wildcard.from_part, name.clone()))
                    .copied()
                    .unwrap_or_default()
                    != 1
                {
                    continue;
                }
                let Some(binding_symbol) =
                    wildcard_alias_symbol(context, wildcard, module_path, name)
                else {
                    continue;
                };
                input
                    .symbol_bindings
                    .push(etas_types::ExternalSymbolBindingInput {
                        symbol: binding_symbol,
                        package: external_package_key(*package),
                        path: external_item_path(module_path, name),
                        span: wildcard.span,
                    });
            }
        }
    }

    input.action_bindings =
        external_effect_action_bindings(hir, &input.symbol_bindings, diagnostics);

    input
}

fn external_package_key(package: ExternalPackageId) -> etas_types::ExternalPackageKey {
    etas_types::ExternalPackageKey(package.0)
}

fn external_item_path(module_path: &ModulePath, name: &str) -> Vec<String> {
    let mut path = module_path.segments.clone();
    path.push(name.to_owned());
    path
}

fn external_effect_action_bindings(
    hir: &etas_hir::HirProgram,
    symbol_bindings: &[etas_types::ExternalSymbolBindingInput],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<etas_types::ExternalActionBindingInput> {
    let action_refs = hir
        .exprs
        .iter()
        .filter_map(|(_, expr)| {
            let etas_hir::HirExpr::Perform { action, .. } = expr else {
                return None;
            };
            Some(action)
        })
        .chain(hir.handler_arms.iter().map(|(_, arm)| &arm.action))
        .collect::<Vec<_>>();
    let mut bindings = Vec::new();
    for action in action_refs {
        let Some(symbol) = external_effect_action_symbol(hir, action) else {
            continue;
        };
        let Some(action_path) = external_effect_action_path(hir, action) else {
            diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                action.span,
                "external effect action requires a resolved import alias path",
            ));
            continue;
        };
        let etas_hir::ResolveResult::Resolved(effect_symbol) = action.effect.path.resolution else {
            diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                action.span,
                "external effect action requires a resolved effect import",
            ));
            continue;
        };
        let Some(effect_binding) = symbol_bindings
            .iter()
            .find(|binding| binding.symbol == effect_symbol)
        else {
            diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                action.span,
                "external effect action requires package-aware import facts",
            ));
            continue;
        };
        bindings.push(etas_types::ExternalActionBindingInput {
            symbol,
            package: effect_binding.package,
            path: action_path,
            span: action.span,
        });
    }
    bindings
}

fn external_effect_action_symbol(
    hir: &etas_hir::HirProgram,
    action: &etas_hir::ResolvedActionRef,
) -> Option<etas_hir::SymbolId> {
    let etas_hir::ResolveResult::Resolved(symbol) = action.action_symbol else {
        return None;
    };
    let symbol_data = hir.symbols.get(symbol)?;
    matches!(
        symbol_data.def,
        etas_hir::SymbolDef::Synthetic {
            reason: etas_hir::SyntheticSymbolReason::ExternalEffectAction
        }
    )
    .then_some(symbol)
}

fn external_effect_action_path(
    hir: &etas_hir::HirProgram,
    action: &etas_hir::ResolvedActionRef,
) -> Option<Vec<String>> {
    let etas_hir::ResolveResult::Resolved(effect_symbol) = action.effect.path.resolution else {
        return None;
    };
    let symbol = hir.symbols.get(effect_symbol)?;
    let etas_hir::SymbolDef::ImportAlias { path, origin } = &symbol.def else {
        return None;
    };
    if *origin != etas_hir::ImportAliasOrigin::SourceImport {
        return None;
    }
    let mut path = path.clone();
    path.push(action.action.clone());
    Some(path)
}

fn convert_external_metadata(
    metadata: &ProjectExternalPublicMetadataInput,
) -> etas_types::ExternalPublicMetadataInput {
    etas_types::ExternalPublicMetadataInput {
        package: external_package_key(metadata.package),
        types: metadata
            .types
            .iter()
            .map(convert_external_named_signature)
            .collect(),
        values: metadata
            .values
            .iter()
            .map(convert_external_named_signature)
            .collect(),
        enums: metadata
            .enums
            .iter()
            .map(convert_external_named_signature)
            .collect(),
        flows: metadata
            .flows
            .iter()
            .map(convert_external_flow_signature)
            .collect(),
        agents: metadata
            .agents
            .iter()
            .map(convert_external_agent_signature)
            .collect(),
        tools: metadata
            .tools
            .iter()
            .map(convert_external_tool_signature)
            .collect(),
        effects: metadata
            .effects
            .iter()
            .map(convert_external_named_signature)
            .collect(),
        trace_specs: metadata
            .trace_specs
            .iter()
            .map(convert_external_named_signature)
            .collect(),
        spec_signatures: metadata
            .spec_signatures
            .iter()
            .map(convert_external_spec_signature)
            .collect(),
        spec_impls: metadata
            .spec_impls
            .iter()
            .map(convert_external_spec_impl)
            .collect(),
        type_spec_satisfactions: metadata
            .type_spec_satisfactions
            .iter()
            .map(convert_external_type_spec_satisfaction)
            .collect(),
        callable_spec_satisfactions: metadata
            .callable_spec_satisfactions
            .iter()
            .map(convert_external_callable_spec_satisfaction)
            .collect(),
        trace_spec_conformances: metadata
            .trace_spec_conformances
            .iter()
            .map(convert_external_trace_spec_conformance)
            .collect(),
        actions: metadata
            .actions
            .iter()
            .map(convert_external_action_signature)
            .collect(),
    }
}

fn convert_external_spec_signature(
    signature: &crate::ProjectExternalSpecSignatureInput,
) -> etas_types::ExternalSpecSignatureInput {
    etas_types::ExternalSpecSignatureInput {
        path: signature.path.clone(),
        kind: convert_external_spec_kind(signature.kind),
        param_names: signature.param_names.clone(),
        callable: signature
            .callable
            .as_ref()
            .map(convert_external_flow_signature),
        methods: signature
            .methods
            .iter()
            .map(|method| etas_types::ExternalSpecMethodInput {
                name: method.name.clone(),
                path: method.path.clone(),
                signature: method
                    .signature
                    .as_ref()
                    .map(convert_external_flow_signature),
            })
            .collect(),
        super_specs: signature
            .super_specs
            .iter()
            .map(|bound| etas_types::ExternalSpecBoundInput {
                spec: bound.spec.clone(),
                args: bound.args.iter().map(convert_external_type).collect(),
            })
            .collect(),
    }
}

fn convert_external_spec_kind(
    kind: ProjectExternalSpecKindInput,
) -> etas_types::ExternalSpecKindInput {
    match kind {
        ProjectExternalSpecKindInput::Type => etas_types::ExternalSpecKindInput::Type,
        ProjectExternalSpecKindInput::Callable => etas_types::ExternalSpecKindInput::Callable,
        ProjectExternalSpecKindInput::Trace => etas_types::ExternalSpecKindInput::Trace,
    }
}

fn convert_external_spec_impl(
    implementation: &crate::ProjectExternalSpecImplInput,
) -> etas_types::ExternalSpecImplInput {
    etas_types::ExternalSpecImplInput {
        self_type: convert_external_type(&implementation.self_type),
        spec: implementation.spec.clone(),
        args: implementation
            .args
            .iter()
            .map(convert_external_type)
            .collect(),
        methods: implementation.methods.clone(),
    }
}

fn convert_external_type_spec_satisfaction(
    fact: &crate::ProjectExternalTypeSpecSatisfactionInput,
) -> etas_types::ExternalTypeSpecSatisfactionInput {
    etas_types::ExternalTypeSpecSatisfactionInput {
        self_type: convert_external_type(&fact.self_type),
        spec: fact.spec.clone(),
        args: fact.args.iter().map(convert_external_type).collect(),
    }
}

fn convert_external_callable_spec_satisfaction(
    fact: &crate::ProjectExternalCallableSpecSatisfactionInput,
) -> etas_types::ExternalCallableSpecSatisfactionInput {
    etas_types::ExternalCallableSpecSatisfactionInput {
        item: fact.item.clone(),
        spec: fact.spec.clone(),
        args: fact.args.iter().map(convert_external_type).collect(),
    }
}

fn convert_external_trace_spec_conformance(
    fact: &crate::ProjectExternalTraceSpecConformanceInput,
) -> etas_types::ExternalTraceSpecConformanceInput {
    etas_types::ExternalTraceSpecConformanceInput {
        item: fact.item.clone(),
        target: match &fact.target {
            crate::ProjectExternalTraceSpecConformanceTargetInput::Inline => {
                etas_types::ExternalTraceSpecConformanceTargetInput::Inline
            }
            crate::ProjectExternalTraceSpecConformanceTargetInput::Named { spec, args } => {
                etas_types::ExternalTraceSpecConformanceTargetInput::Named {
                    spec: spec.clone(),
                    args: args.iter().map(convert_external_type).collect(),
                }
            }
        },
    }
}

fn convert_external_named_signature(
    signature: &ProjectExternalNamedSignatureInput,
) -> etas_types::ExternalNamedSignatureInput {
    etas_types::ExternalNamedSignatureInput {
        path: signature.path.clone(),
        ty: signature.ty.as_ref().map(convert_external_type),
    }
}

fn convert_external_flow_signature(
    signature: &ProjectExternalFlowSignatureInput,
) -> etas_types::ExternalFlowSignatureInput {
    etas_types::ExternalFlowSignatureInput {
        path: signature.path.clone(),
        generic_params: convert_external_generic_params(&signature.generic_params),
        params: signature.params.iter().map(convert_external_type).collect(),
        output: convert_external_type(&signature.output),
        effects: signature.effects.as_ref().map(convert_external_effect_row),
    }
}

fn convert_external_agent_signature(
    signature: &ProjectExternalAgentSignatureInput,
) -> etas_types::ExternalAgentSignatureInput {
    etas_types::ExternalAgentSignatureInput {
        path: signature.path.clone(),
        generic_params: convert_external_generic_params(&signature.generic_params),
        input: signature.input.iter().map(convert_external_type).collect(),
        output: convert_external_type(&signature.output),
        effects: signature.effects.as_ref().map(convert_external_effect_row),
    }
}

fn convert_external_tool_signature(
    signature: &ProjectExternalToolSignatureInput,
) -> etas_types::ExternalToolSignatureInput {
    etas_types::ExternalToolSignatureInput {
        path: signature.path.clone(),
        generic_params: convert_external_generic_params(&signature.generic_params),
        input: signature.input.iter().map(convert_external_type).collect(),
        output: convert_external_type(&signature.output),
        effects: signature.effects.as_ref().map(convert_external_effect_row),
    }
}

fn convert_external_action_signature(
    signature: &ProjectExternalActionSignatureInput,
) -> etas_types::ExternalActionSignatureInput {
    etas_types::ExternalActionSignatureInput {
        path: signature.path.clone(),
        generic_params: convert_external_generic_params(&signature.generic_params),
        params: signature.params.iter().map(convert_external_type).collect(),
        effect_args: signature
            .effect_args
            .iter()
            .map(convert_external_action_arg_kind)
            .collect(),
        selector_param_names: signature.selector_param_names.clone(),
        selector_defaults: signature
            .selector_defaults
            .iter()
            .map(|arg| arg.as_ref().map(convert_external_effect_arg))
            .collect(),
        output: convert_external_type(&signature.output),
        returns_never: signature.returns_never,
    }
}

fn convert_external_generic_params(
    params: &[crate::ProjectExternalCallableGenericParamInput],
) -> Vec<etas_types::ExternalCallableGenericParamInput> {
    params
        .iter()
        .map(|param| etas_types::ExternalCallableGenericParamInput {
            name: param.name.clone(),
            bounds: param
                .bounds
                .iter()
                .map(|bound| etas_types::ExternalSpecBoundInput {
                    spec: bound.spec.clone(),
                    args: bound.args.iter().map(convert_external_type).collect(),
                })
                .collect(),
        })
        .collect()
}

fn convert_external_action_arg_kind(
    kind: &ProjectExternalActionArgKindInput,
) -> etas_types::ExternalActionArgKindInput {
    match kind {
        ProjectExternalActionArgKindInput::Type => etas_types::ExternalActionArgKindInput::Type,
        ProjectExternalActionArgKindInput::MemoryPlace => {
            etas_types::ExternalActionArgKindInput::MemoryPlace
        }
        ProjectExternalActionArgKindInput::StaticResourcePath { ty } => {
            etas_types::ExternalActionArgKindInput::StaticResourcePath { ty: ty.clone() }
        }
        ProjectExternalActionArgKindInput::StringPattern => {
            etas_types::ExternalActionArgKindInput::StringPattern
        }
    }
}

pub(super) fn convert_external_effect_row(
    row: &ProjectExternalEffectRowInput,
) -> etas_types::ExternalEffectRowInput {
    etas_types::ExternalEffectRowInput {
        effects: row
            .effects
            .iter()
            .map(convert_external_effect_ref)
            .collect(),
    }
}

fn convert_external_effect_ref(
    effect: &ProjectExternalEffectRefInput,
) -> etas_types::ExternalEffectRefInput {
    etas_types::ExternalEffectRefInput {
        path: effect.path.clone(),
        args: effect
            .args
            .iter()
            .map(convert_external_effect_arg)
            .collect(),
    }
}

pub(super) fn convert_external_effect_arg(
    arg: &ProjectExternalEffectArgInput,
) -> etas_types::ExternalEffectArgInput {
    match arg {
        ProjectExternalEffectArgInput::Type(ty) => {
            etas_types::ExternalEffectArgInput::Type(convert_external_type(ty))
        }
        ProjectExternalEffectArgInput::Path(path) => {
            etas_types::ExternalEffectArgInput::Path(path.clone())
        }
        ProjectExternalEffectArgInput::String(value) => {
            etas_types::ExternalEffectArgInput::String(value.clone())
        }
        ProjectExternalEffectArgInput::Int(value) => {
            etas_types::ExternalEffectArgInput::Int(value.clone())
        }
        ProjectExternalEffectArgInput::Wildcard => etas_types::ExternalEffectArgInput::Wildcard,
    }
}

fn convert_external_type(ty: &ProjectExternalTypeInput) -> etas_types::ExternalTypeInput {
    match ty {
        ProjectExternalTypeInput::Primitive(name) => {
            etas_types::ExternalTypeInput::Primitive(name.clone())
        }
        ProjectExternalTypeInput::Var(name) => etas_types::ExternalTypeInput::Var(name.clone()),
        ProjectExternalTypeInput::Named(path) => etas_types::ExternalTypeInput::Named(path.clone()),
        ProjectExternalTypeInput::Applied { path, args } => {
            etas_types::ExternalTypeInput::Applied {
                path: path.clone(),
                args: args.iter().map(convert_external_type).collect(),
            }
        }
        ProjectExternalTypeInput::Alias { path, target } => etas_types::ExternalTypeInput::Alias {
            path: path.clone(),
            target: Box::new(convert_external_type(target)),
        },
        ProjectExternalTypeInput::Nominal {
            path,
            representation,
        } => etas_types::ExternalTypeInput::Nominal {
            path: path.clone(),
            representation: representation
                .as_ref()
                .map(|representation| Box::new(convert_external_type(representation))),
        },
        ProjectExternalTypeInput::Array(element) => {
            etas_types::ExternalTypeInput::Array(Box::new(convert_external_type(element)))
        }
        ProjectExternalTypeInput::List(element) => {
            etas_types::ExternalTypeInput::List(Box::new(convert_external_type(element)))
        }
        ProjectExternalTypeInput::Map { key, value } => etas_types::ExternalTypeInput::Map {
            key: Box::new(convert_external_type(key)),
            value: Box::new(convert_external_type(value)),
        },
        ProjectExternalTypeInput::Set(element) => {
            etas_types::ExternalTypeInput::Set(Box::new(convert_external_type(element)))
        }
        ProjectExternalTypeInput::Range(index) => {
            etas_types::ExternalTypeInput::Range(Box::new(convert_external_type(index)))
        }
        ProjectExternalTypeInput::Slice(element) => {
            etas_types::ExternalTypeInput::Slice(Box::new(convert_external_type(element)))
        }
        ProjectExternalTypeInput::Option(inner) => {
            etas_types::ExternalTypeInput::Option(Box::new(convert_external_type(inner)))
        }
        ProjectExternalTypeInput::Result { ok, err } => etas_types::ExternalTypeInput::Result {
            ok: Box::new(convert_external_type(ok)),
            err: Box::new(convert_external_type(err)),
        },
        ProjectExternalTypeInput::Record { fields } => etas_types::ExternalTypeInput::Record {
            fields: fields.iter().map(convert_external_record_field).collect(),
        },
        ProjectExternalTypeInput::Tuple(elements) => etas_types::ExternalTypeInput::Tuple(
            elements.iter().map(convert_external_type).collect(),
        ),
        ProjectExternalTypeInput::Function {
            input,
            output,
            effects,
        } => etas_types::ExternalTypeInput::Function {
            input: input.iter().map(convert_external_type).collect(),
            output: Box::new(convert_external_type(output)),
            effects: effects.as_ref().map(convert_external_effect_row),
        },
        ProjectExternalTypeInput::Handler {
            handled,
            produced,
            result,
        } => etas_types::ExternalTypeInput::Handler {
            handled: convert_external_effect_row(handled),
            produced: produced.as_ref().map(convert_external_effect_row),
            result: result
                .as_ref()
                .map(|result| Box::new(convert_external_type(result))),
        },
        ProjectExternalTypeInput::Trust { wrapper, inner } => {
            etas_types::ExternalTypeInput::Trust {
                wrapper: wrapper.clone(),
                inner: Box::new(convert_external_type(inner)),
            }
        }
        ProjectExternalTypeInput::Prompt => etas_types::ExternalTypeInput::Prompt,
        ProjectExternalTypeInput::PromptPart => etas_types::ExternalTypeInput::PromptPart,
        ProjectExternalTypeInput::Message(inner) => {
            etas_types::ExternalTypeInput::Message(Box::new(convert_external_type(inner)))
        }
        ProjectExternalTypeInput::MemorySelection(inner) => {
            etas_types::ExternalTypeInput::MemorySelection(Box::new(convert_external_type(inner)))
        }
        ProjectExternalTypeInput::Store { key, value } => etas_types::ExternalTypeInput::Store {
            key: Box::new(convert_external_type(key)),
            value: Box::new(convert_external_type(value)),
        },
        ProjectExternalTypeInput::MemoryRegion(schema) => {
            etas_types::ExternalTypeInput::MemoryRegion(Box::new(convert_external_type(schema)))
        }
        ProjectExternalTypeInput::ResourceHandle { name, args } => {
            etas_types::ExternalTypeInput::ResourceHandle {
                name: name.clone(),
                args: args.iter().map(convert_external_type).collect(),
            }
        }
    }
}

fn convert_external_record_field(
    field: &ProjectExternalRecordFieldInput,
) -> etas_types::ExternalRecordFieldInput {
    etas_types::ExternalRecordFieldInput {
        name: field.name.clone(),
        ty: convert_external_type(&field.ty),
    }
}

fn std_signature_input(
    context: &ProjectContext,
    hir: &etas_hir::HirProgram,
) -> etas_types::StdSignatureInput {
    let registry = context.std_registry.as_ref();
    let mut input = etas_types::StdSignatureInput::default();

    if let Some(resolved_imports) = context.resolved_imports.as_ref() {
        for resolved in &resolved_imports.imports {
            let ImportTarget::StdItem { symbol, .. } = &resolved.target else {
                continue;
            };
            let Some(std_symbol) = registry.symbol(*symbol) else {
                continue;
            };
            let Some(binding_symbol) = hir_module_for_source_module(context, resolved.from)
                .and_then(|hir_module| hir.modules_arena.get(hir_module))
                .and_then(|module| {
                    import_binding_symbol_for_import(
                        module,
                        resolved.import.source,
                        resolved.span,
                        &resolved.local_name,
                    )
                })
            else {
                continue;
            };
            input
                .symbol_bindings
                .push(etas_types::StdSymbolBindingInput {
                    symbol: binding_symbol,
                    qualified_path: std_symbol.qualified_path.clone(),
                });
        }
    }

    if let Some(resolved_imports) = context.resolved_imports.as_ref() {
        let wildcard_counts = wildcard_import_name_counts(&resolved_imports.wildcard_imports);
        for wildcard in &resolved_imports.wildcard_imports {
            let ResolvedModuleTarget::Std { module, path, .. } = &wildcard.target_module else {
                continue;
            };
            for name in &wildcard.exported_names {
                if wildcard_counts
                    .get(&(wildcard.from, wildcard.from_part, name.clone()))
                    .copied()
                    .unwrap_or_default()
                    != 1
                {
                    continue;
                }
                let Some(std_symbol) = registry
                    .symbols()
                    .find(|symbol| symbol.module == *module && symbol.name == *name)
                else {
                    continue;
                };
                let Some(binding_symbol) = wildcard_alias_symbol(context, wildcard, path, name)
                else {
                    continue;
                };
                input
                    .symbol_bindings
                    .push(etas_types::StdSymbolBindingInput {
                        symbol: binding_symbol,
                        qualified_path: std_symbol.qualified_path.clone(),
                    });
            }
        }
    }

    for symbol in hir.symbols.iter() {
        let etas_hir::SymbolDef::ImportAlias { path, origin } = &symbol.def else {
            continue;
        };
        if *origin != etas_hir::ImportAliasOrigin::StdPrelude {
            continue;
        }
        let Some(std_symbol) = registry.lookup_qualified(path) else {
            continue;
        };
        input
            .symbol_bindings
            .push(etas_types::StdSymbolBindingInput {
                symbol: symbol.id,
                qualified_path: std_symbol.qualified_path.clone(),
            });
    }

    input
}

fn hir_module_for_source_module(
    context: &ProjectContext,
    source_module: ModuleId,
) -> Option<etas_hir::HirModuleId> {
    let hir = context.hir.as_ref().map(|output| &output.hir)?;
    let modules = context.modules.as_ref()?;
    let module_info = modules.modules.get(source_module)?;
    hir.modules.iter().find_map(|hir_module| {
        let hir_module_data = hir.modules_arena.get(*hir_module)?;
        let path = hir_module_data.name.as_ref()?;
        (path
            .segments
            .iter()
            .map(|segment| segment.name.as_str())
            .eq(module_info.path.segments.iter().map(String::as_str)))
        .then_some(*hir_module)
    })
}

fn source_signature_input(context: &ProjectContext) -> etas_types::SourceSignatureInput {
    let Some(hir) = context.hir.as_ref().map(|output| &output.hir) else {
        return etas_types::SourceSignatureInput::default();
    };
    let Some(modules) = context.modules.as_ref() else {
        return etas_types::SourceSignatureInput::default();
    };
    let Some(resolved_imports) = context.resolved_imports.as_ref() else {
        return etas_types::SourceSignatureInput::default();
    };
    let Some(bindings) = context.hir_item_bindings.as_ref() else {
        return etas_types::SourceSignatureInput::default();
    };
    let mut input = etas_types::SourceSignatureInput::default();
    let module_to_hir = modules
        .modules
        .iter()
        .filter_map(|(module_id, module_info)| {
            let hir_module = hir.modules.iter().find_map(|hir_module| {
                let hir_module_data = hir.modules_arena.get(*hir_module)?;
                let path = hir_module_data.name.as_ref()?;
                (path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(module_info.path.segments.iter().map(String::as_str)))
                .then_some(*hir_module)
            })?;
            Some((module_id, hir_module))
        })
        .collect::<HashMap<ModuleId, etas_hir::HirModuleId>>();

    for resolved in &resolved_imports.imports {
        let ImportTarget::SourceItem { item, .. } = &resolved.target else {
            continue;
        };
        let Some(target_hir_item) = bindings.ast_to_hir.get(item).copied() else {
            continue;
        };
        let Some(target_symbol) = item_symbol(hir, target_hir_item) else {
            continue;
        };
        let Some(hir_module) = module_to_hir.get(&resolved.from).copied() else {
            continue;
        };
        let Some(binding_symbol) = hir.modules_arena.get(hir_module).and_then(|module| {
            import_binding_symbol_for_import(
                module,
                resolved.import.source,
                resolved.span,
                &resolved.local_name,
            )
        }) else {
            continue;
        };
        input
            .symbol_bindings
            .push(etas_types::SourceSymbolBindingInput {
                symbol: binding_symbol,
                target: target_symbol,
            });
    }

    let wildcard_counts = wildcard_import_name_counts(&resolved_imports.wildcard_imports);
    for wildcard in &resolved_imports.wildcard_imports {
        let ResolvedModuleTarget::Source { module, .. } = &wildcard.target_module else {
            continue;
        };
        let Some(target_module) = modules.modules.get(*module) else {
            continue;
        };
        for name in &wildcard.exported_names {
            if wildcard_counts
                .get(&(wildcard.from, wildcard.from_part, name.clone()))
                .copied()
                .unwrap_or_default()
                != 1
            {
                continue;
            }
            let Some(export) = target_module.visibility_exports.items.get(name) else {
                continue;
            };
            let Some(target_hir_item) = bindings.ast_to_hir.get(&export.item).copied() else {
                continue;
            };
            let Some(target_symbol) = item_symbol(hir, target_hir_item) else {
                continue;
            };
            let Some(binding_symbol) =
                wildcard_alias_symbol(context, wildcard, &target_module.path, name)
            else {
                continue;
            };
            input
                .symbol_bindings
                .push(etas_types::SourceSymbolBindingInput {
                    symbol: binding_symbol,
                    target: target_symbol,
                });
        }
    }
    input
}

fn item_symbol(
    hir: &etas_hir::HirProgram,
    item: etas_hir::HirItemId,
) -> Option<etas_hir::SymbolId> {
    match hir.items.get(item)? {
        etas_hir::HirItem::TypeAlias(item) => Some(item.symbol),
        etas_hir::HirItem::Type(item) => Some(item.symbol),
        etas_hir::HirItem::Enum(item) => Some(item.symbol),
        etas_hir::HirItem::Spec(item) => Some(item.symbol),
        etas_hir::HirItem::Effect(item) => Some(item.symbol),
        etas_hir::HirItem::TopLevelLet(item) => Some(item.symbol),
        etas_hir::HirItem::Tool(item) => Some(item.symbol),
        etas_hir::HirItem::Agent(item) => Some(item.symbol),
        etas_hir::HirItem::Protocol(item) => Some(item.symbol),
        etas_hir::HirItem::Flow(item) => Some(item.symbol),
        etas_hir::HirItem::Impl(_) | etas_hir::HirItem::Error { .. } => None,
    }
}

fn import_binding_symbol_for_import(
    module: &etas_hir::HirModule,
    source: etas_core::SourceId,
    span: etas_core::Span,
    local_name: &str,
) -> Option<etas_hir::SymbolId> {
    module.imports.iter().find_map(|import| {
        let binding = import.binding.as_ref()?;
        (binding.local_name == local_name && import.span.source == source && import.span == span)
            .then_some(binding.symbol)
    })
}

fn wildcard_alias_symbol(
    context: &ProjectContext,
    wildcard: &ResolvedWildcardImport,
    target_module_path: &ModulePath,
    name: &str,
) -> Option<etas_hir::SymbolId> {
    let hir = context.hir.as_ref().map(|output| &output.hir)?;
    let modules = context.modules.as_ref()?;
    let part = modules.parts.get(wildcard.from_part)?;
    let hir_module = hir_module_for_source_module(context, wildcard.from)?;
    let part_scope = hir_part_scope(hir, hir_module, part.source)?;
    let symbol = hir.scopes.lookup_local(part_scope, name)?;
    let symbol_data = hir.symbols.get(symbol)?;
    let etas_hir::SymbolDef::ImportAlias { path, origin } = &symbol_data.def else {
        return None;
    };
    if *origin != etas_hir::ImportAliasOrigin::SourceImport {
        return None;
    }
    let mut expected_path = target_module_path.segments.clone();
    expected_path.push(name.to_owned());
    (path == &expected_path).then_some(symbol)
}

fn hir_part_scope(
    hir: &etas_hir::HirProgram,
    hir_module: etas_hir::HirModuleId,
    source: etas_core::SourceId,
) -> Option<etas_hir::ScopeId> {
    let module_scope = hir.modules_arena.get(hir_module)?.scope;
    hir.scopes.iter().find_map(|scope| {
        (scope.owner == etas_hir::ScopeOwner::Module(hir_module)
            && scope.parent == Some(module_scope)
            && scope.span.source == source)
            .then_some(scope.id)
    })
}

fn wildcard_import_name_counts(
    wildcard_imports: &[ResolvedWildcardImport],
) -> HashMap<(ModuleId, crate::ModulePartId, String), usize> {
    let mut counts = HashMap::new();
    for wildcard in wildcard_imports {
        for name in &wildcard.exported_names {
            *counts
                .entry((wildcard.from, wildcard.from_part, name.clone()))
                .or_insert(0) += 1;
        }
    }
    counts
}
