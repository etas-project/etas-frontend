use crate::{
    StdSignatureInput, StdSymbolBindingInput, SymbolTypeFact,
    lower::std::{lower_std_spec_impls, lower_std_symbol, lower_std_type_symbol},
    pipeline::context::TypePipelineContext,
};

pub fn apply_hir_std_prelude(ctx: &mut TypePipelineContext<'_>) {
    let bindings = ctx
        .hir
        .symbols
        .iter()
        .filter_map(|symbol| {
            let etas_hir::SymbolDef::ImportAlias { path, origin } = &symbol.def else {
                return None;
            };
            (*origin == etas_hir::ImportAliasOrigin::StdPrelude
                || path.first().is_some_and(|segment| segment == "std"))
            .then(|| StdSymbolBindingInput {
                symbol: symbol.id,
                qualified_path: path.clone(),
            })
        })
        .collect::<Vec<_>>();
    apply_std_signature_input(
        ctx,
        &StdSignatureInput {
            symbol_bindings: bindings,
        },
    );
}

pub fn apply_std_signature_input(ctx: &mut TypePipelineContext<'_>, input: &StdSignatureInput) {
    let registry = ctx.std_registry.clone();
    record_known_std_types(ctx, &registry);
    ctx.signature_facts.std_spec_impls = lower_std_spec_impls(ctx, &registry);
    for binding in &input.symbol_bindings {
        let Some(symbol) = registry.lookup_qualified(&binding.qualified_path) else {
            continue;
        };
        let fact = lower_std_symbol(ctx, &registry, binding.symbol, symbol);
        if matches!(
            symbol.decl,
            etas_std::StdDecl::Type(etas_std::TypeDecl {
                kind: etas_std::TypeDeclKind::Spec,
                ..
            })
        ) {
            ctx.signature_facts
                .std_spec_aliases
                .insert(binding.symbol, symbol.qualified_path.clone());
        }
        if let SymbolTypeFact::EffectAction { signature } = &fact {
            ctx.signature_facts
                .action_signatures
                .insert(binding.symbol, signature.clone());
        }
        ctx.signature_facts
            .symbol_types
            .insert(binding.symbol, fact);
    }
}

fn record_known_std_types(ctx: &mut TypePipelineContext<'_>, registry: &etas_std::StdRegistry) {
    ctx.signature_facts.known_std_types.priority_queue = registry
        .lookup_qualified(&["std", "collections", "PriorityQueue"])
        .and_then(|symbol| lower_std_type_symbol(ctx, registry, symbol));
    ctx.signature_facts.known_std_types.ordered_map = registry
        .lookup_qualified(&["std", "collections", "OrderedMap"])
        .and_then(|symbol| lower_std_type_symbol(ctx, registry, symbol));
    ctx.signature_facts.known_std_types.ordered_set = registry
        .lookup_qualified(&["std", "collections", "OrderedSet"])
        .and_then(|symbol| lower_std_type_symbol(ctx, registry, symbol));
    for symbol in registry.symbols() {
        if let etas_std::StdDecl::Type(declaration) = &symbol.decl
            && let Some(role) = declaration.runtime_role
            && let Some(ty) = lower_std_type_symbol(ctx, registry, symbol)
        {
            ctx.signature_facts
                .known_std_types
                .runtime_types
                .insert(role, ty);
        }
        if let etas_std::StdDecl::Type(declaration) = &symbol.decl
            && !declaration.projected_fields.is_empty()
        {
            let fields = declaration
                .projected_fields
                .iter()
                .map(|field| crate::FieldType {
                    name: field.name.clone(),
                    ty: crate::lower::std::lower_std_type(ctx, registry, &field.ty),
                })
                .collect::<Vec<_>>();
            let local = crate::lower::std::lower_std_type_decl(ctx, registry, declaration);
            ctx.signature_facts
                .known_std_types
                .projected_fields
                .insert(local, fields.clone());
            if let Some(canonical) = lower_std_type_symbol(ctx, registry, symbol) {
                ctx.signature_facts
                    .known_std_types
                    .projected_fields
                    .insert(canonical, fields);
            }
        }
        if let etas_std::StdDecl::Type(declaration) = &symbol.decl
            && declaration.construction == etas_std::StdTypeConstruction::IntrinsicOnly
        {
            let local = crate::lower::std::lower_std_type_decl(ctx, registry, declaration);
            ctx.signature_facts
                .known_std_types
                .intrinsic_only_constructors
                .insert(local);
            if let Some(canonical) = lower_std_type_symbol(ctx, registry, symbol) {
                ctx.signature_facts
                    .known_std_types
                    .intrinsic_only_constructors
                    .insert(canonical);
            }
        }
        if let etas_std::StdDecl::Type(declaration) = &symbol.decl
            && declaration.kind == etas_std::TypeDeclKind::Support
            && !declaration.derivable
        {
            let local = crate::lower::std::lower_std_type_decl(ctx, registry, declaration);
            ctx.signature_facts
                .known_std_types
                .opaque_values
                .insert(local);
            if let Some(canonical) = lower_std_type_symbol(ctx, registry, symbol) {
                ctx.signature_facts
                    .known_std_types
                    .opaque_values
                    .insert(canonical);
            }
        }
        if let etas_std::StdDecl::Type(declaration) = &symbol.decl
            && let Some(role) = declaration.code_role
            && let Some(ty) = lower_std_type_symbol(ctx, registry, symbol)
        {
            match role {
                etas_std::CodeTypeRole::AgentId => {
                    ctx.signature_facts.known_std_types.code.agent_id = Some(ty)
                }
                etas_std::CodeTypeRole::TypeInfo => {
                    ctx.signature_facts.known_std_types.code.type_info = Some(ty)
                }
                etas_std::CodeTypeRole::GenericOwnerInfo => {
                    ctx.signature_facts.known_std_types.code.generic_owner_info = Some(ty)
                }
                etas_std::CodeTypeRole::ToolRef => {
                    ctx.signature_facts.known_std_types.code.tool_ref = Some(ty)
                }
                etas_std::CodeTypeRole::StaticValueInfo => {
                    ctx.signature_facts.known_std_types.code.static_value_info = Some(ty)
                }
                etas_std::CodeTypeRole::TraceConstraintInfo => {
                    ctx.signature_facts
                        .known_std_types
                        .code
                        .trace_constraint_info = Some(ty)
                }
                etas_std::CodeTypeRole::AgentDefinition => {
                    ctx.signature_facts.known_std_types.code.agent_definition = Some(ty)
                }
                etas_std::CodeTypeRole::CallableDefinition => {
                    ctx.signature_facts.known_std_types.code.callable_definition = Some(ty)
                }
                etas_std::CodeTypeRole::SyntaxBlock => {
                    ctx.signature_facts.known_std_types.code.syntax_block = Some(ty)
                }
                etas_std::CodeTypeRole::DefinitionId => {
                    ctx.signature_facts.known_std_types.code.definition_id = Some(ty)
                }
                etas_std::CodeTypeRole::CallableId => {
                    ctx.signature_facts.known_std_types.code.callable_id = Some(ty)
                }
                etas_std::CodeTypeRole::EffectRowInfo => {
                    ctx.signature_facts.known_std_types.code.effect_row_info = Some(ty)
                }
                etas_std::CodeTypeRole::ActionSummary => {
                    ctx.signature_facts.known_std_types.code.action_summary = Some(ty)
                }
                etas_std::CodeTypeRole::TraceSummary => {
                    ctx.signature_facts.known_std_types.code.trace_summary = Some(ty)
                }
                etas_std::CodeTypeRole::Determinism => {
                    ctx.signature_facts.known_std_types.code.determinism = Some(ty)
                }
                etas_std::CodeTypeRole::DescriptorDetail => {
                    ctx.signature_facts.known_std_types.code.descriptor_detail = Some(ty)
                }
                etas_std::CodeTypeRole::TraceWitness => {
                    ctx.signature_facts.known_std_types.code.trace_witness = Some(ty)
                }
            }
        }
        if let etas_std::StdDecl::Type(declaration) = &symbol.decl
            && let Some(item) = declaration.iterable_item
            && let Some(constructor) = lower_std_type_symbol(ctx, registry, symbol)
        {
            ctx.signature_facts.known_std_types.iterables.insert(
                constructor,
                crate::facts::symbols::StdIterableFact {
                    arity: declaration.params.len(),
                    item: match item {
                        etas_std::StdIterableItem::Element { parameter } => {
                            crate::facts::symbols::StdIterableItemFact::Element { parameter }
                        }
                        etas_std::StdIterableItem::Entry { key, value } => {
                            crate::facts::symbols::StdIterableItemFact::Entry { key, value }
                        }
                    },
                },
            );
        }
    }
    for kind in [
        etas_std::StdSupportConstraint::Index,
        etas_std::StdSupportConstraint::LengthInput,
        etas_std::StdSupportConstraint::EmptinessInput,
    ] {
        let ty =
            crate::lower::std::lower_std_type(ctx, registry, &etas_std::StdType::Support(kind));
        ctx.signature_facts
            .known_std_types
            .support_constraints
            .insert(ty);
    }
    let Some(index_error) = registry
        .lookup_qualified(&["std", "runtime", "error", "IndexError"])
        .and_then(|symbol| lower_std_type_symbol(ctx, registry, symbol))
    else {
        return;
    };
    ctx.signature_facts.known_std_types.index_error = Some(index_error);
}
