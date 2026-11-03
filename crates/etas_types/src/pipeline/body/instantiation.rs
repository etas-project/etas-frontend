use std::{collections::HashMap, sync::Arc};

use etas_hir::{HirItem, HirItemId, HirProgram, SymbolDef, SymbolId};

use crate::{
    BodyInstantiationError, CallableGenericArg, CallableGenericParamKind, CallableSignature,
    EffectRowRef, InstantiatedBodyOutput, ItemSignature, SignatureEnvironment, SpecObligation,
    SymbolTypeFact, Type, TypeId, TypeSubstitutionEngine,
    pipeline::{body, context::TypePipelineContext},
};

pub(crate) struct BodyParameters {
    pub owner: HirItemId,
    pub types: HashMap<SymbolId, TypeId>,
    pub rows: HashMap<SymbolId, EffectRowRef>,
    pub row_names: HashMap<String, EffectRowRef>,
    pub signature: ItemSignature,
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    pub obligations: Vec<SpecObligation>,
}

/// Arguments are closed types/rows in `base`'s universe; its template stays unchanged.
pub fn check_instantiated_body_delta(
    hir: &HirProgram,
    base: &SignatureEnvironment,
    item: HirItemId,
    arguments: &[CallableGenericArg],
    std_registry: Arc<etas_std::StdRegistry>,
) -> Result<InstantiatedBodyOutput, BodyInstantiationError> {
    check(hir, base, item, arguments, std_registry, None)
}

pub fn check_instantiated_body_delta_controlled(
    hir: &HirProgram,
    base: &SignatureEnvironment,
    item: HirItemId,
    arguments: &[CallableGenericArg],
    std_registry: Arc<etas_std::StdRegistry>,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<InstantiatedBodyOutput, BodyInstantiationError> {
    check(hir, base, item, arguments, std_registry, Some(control))
}

fn check(
    hir: &HirProgram,
    base: &SignatureEnvironment,
    item: HirItemId,
    arguments: &[CallableGenericArg],
    std_registry: Arc<etas_std::StdRegistry>,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<InstantiatedBodyOutput, BodyInstantiationError> {
    if let Some(control) = control {
        control
            .checkpoint()
            .map_err(BodyInstantiationError::Interrupted)?;
    }
    let mut ctx = TypePipelineContext::for_body(hir, base, std_registry);
    let parameters = BodyParameters::prepare(&mut ctx, item, arguments)?;
    let ItemSignature::Flow(signature) = &parameters.signature else {
        return Err(BodyInstantiationError::UnsupportedOwner(item));
    };
    let signature = signature.clone();
    ctx.body_parameters = Some(parameters);
    match control {
        Some(control) => body::run_controlled(&mut ctx, item, control)
            .map_err(BodyInstantiationError::Interrupted)?,
        None => body::run(&mut ctx, item),
    }
    let body = crate::pipeline::body_delta_output(ctx, base, item);
    Ok(InstantiatedBodyOutput::new(
        body,
        arguments.to_vec(),
        signature,
    ))
}

impl BodyParameters {
    fn prepare(
        ctx: &mut TypePipelineContext<'_>,
        owner: HirItemId,
        arguments: &[CallableGenericArg],
    ) -> Result<Self, BodyInstantiationError> {
        let Some(HirItem::Flow(flow)) = ctx.hir.items.get(owner) else {
            return Err(BodyInstantiationError::UnsupportedOwner(owner));
        };
        let Some(ItemSignature::Flow(template)) = ctx.item_signature(owner).cloned() else {
            return Err(BodyInstantiationError::MissingSignature(owner));
        };
        if flow.params.len() != template.params.len() {
            return Err(BodyInstantiationError::InvalidSignature(owner));
        }
        if template.generic_params.len() != arguments.len() {
            return Err(BodyInstantiationError::ArgumentCount {
                expected: template.generic_params.len(),
                actual: arguments.len(),
            });
        }
        if flow.type_params.len() != arguments.len() {
            return Err(BodyInstantiationError::InvalidBinder {
                position: arguments.len(),
            });
        }
        let mut types = HashMap::new();
        let mut rows = HashMap::new();
        let mut row_names = HashMap::new();
        for (position, ((parameter, symbol), argument)) in template
            .generic_params
            .iter()
            .zip(&flow.type_params)
            .zip(arguments)
            .enumerate()
        {
            let source = ctx
                .hir
                .symbols
                .get(*symbol)
                .ok_or(BodyInstantiationError::InvalidBinder { position })?;
            if source.name != parameter.name
                || !matches!(ctx.interner.store().get(parameter.subject), Some(Type::Named(name))
                    if name.source_parameter == Some(*symbol))
            {
                return Err(BodyInstantiationError::InvalidBinder { position });
            }
            match (parameter.kind, &source.def, argument) {
                (
                    CallableGenericParamKind::Type,
                    SymbolDef::TypeParam { .. },
                    CallableGenericArg::Type(ty),
                ) => {
                    if ctx.interner.store().get(*ty).is_none() {
                        return Err(BodyInstantiationError::MissingType(*ty));
                    }
                    types.insert(*symbol, *ty);
                }
                (
                    CallableGenericParamKind::Effect,
                    SymbolDef::EffectParam { .. },
                    CallableGenericArg::EffectRow(row),
                ) => {
                    rows.insert(*symbol, row.clone());
                    row_names.insert(parameter.name.clone(), row.clone());
                }
                _ => return Err(BodyInstantiationError::ArgumentKind { position }),
            }
        }
        let names = HashMap::new();
        // Validate even arguments absent from the callable's parameter/result
        // types. A phantom parameter cannot hide a malformed type graph.
        let mut argument_types =
            TypeSubstitutionEngine::materializing(&mut ctx.interner, &names, None);
        for argument in arguments {
            match argument {
                CallableGenericArg::Type(ty) => {
                    argument_types.validate_closed_argument(*ty)?;
                    argument_types.substitute(*ty)?;
                }
                CallableGenericArg::EffectRow(row) => {
                    argument_types.validate_closed_effect_argument(row)?;
                    argument_types.substitute_effect_row(row.clone())?;
                }
            }
        }
        let mut engine = TypeSubstitutionEngine::materializing(&mut ctx.interner, &names, None)
            .with_source_parameters(&types)
            .with_effect_arguments(&row_names);
        let mut obligations = Vec::new();
        for parameter in &template.generic_params {
            for bound in &parameter.bounds {
                obligations.push(SpecObligation {
                    ty: engine.substitute(parameter.subject)?,
                    spec: bound.spec.clone(),
                    args: bound
                        .args
                        .iter()
                        .map(|ty| engine.substitute(*ty))
                        .collect::<Result<_, _>>()?,
                    span: flow.span,
                });
            }
        }
        let signature = CallableSignature {
            generic_params: Vec::new(),
            params: template
                .params
                .iter()
                .map(|ty| engine.substitute(*ty))
                .collect::<Result<_, _>>()?,
            output: engine.substitute(template.output)?,
            effects: template
                .effects
                .map(|row| engine.substitute_effect_row(row))
                .transpose()?,
            requested_actions: template
                .requested_actions
                .map(|row| engine.substitute_effect_row(row))
                .transpose()?,
        };
        let symbol_types = flow
            .params
            .iter()
            .zip(&signature.params)
            .map(|(symbol, ty)| (*symbol, SymbolTypeFact::Param { ty: *ty }))
            .collect::<HashMap<_, _>>();
        Ok(Self {
            owner,
            types,
            rows,
            row_names,
            signature: ItemSignature::Flow(signature),
            symbol_types,
            obligations,
        })
    }
}
