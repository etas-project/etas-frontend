use super::{
    descriptor::{DescriptorInput, ProjectionError},
    identity::DefinitionIdentityIndex,
};
use etas_code::{
    CodeFingerprint,
    descriptor::{ParameterDefaultInfo, ParameterDefaultOwner},
};
use etas_hir::{
    HirImplItem, HirImplTarget, HirItem, HirSpecItem, ResolveResult, SymbolDef, SymbolId,
};

/// A body coordinate in its defining program. This archive record is not an
/// executable permit; library restoration rebuilds and compares the index.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ParameterDefaultUnit {
    pub parameter: SymbolId,
    pub owner: etas_hir::HirItemId,
    pub expression: etas_hir::HirExprId,
    pub scope: etas_hir::ScopeId,
    pub output: etas_types::TypeId,
}

pub fn for_program(
    checked: &crate::CheckedProgramData,
) -> Result<std::collections::BTreeMap<ParameterDefaultInfo, ParameterDefaultUnit>, ProjectionError>
{
    if checked.hir.parameter_defaults.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }
    let origins = crate::code::identity::DefinitionOrigins {
        source_modules: crate::code::identity::DefinitionOrigins::checked_source_modules(checked)?,
        standard_package: crate::code::identity::standard_package_key(&checked.std_registry)?,
        external_definitions:
            crate::code::identity::DefinitionOrigins::checked_external_definitions(checked)?,
    };
    let identities = DefinitionIdentityIndex::build(checked, &origins)?;
    collect_units(checked.into(), &identities)
}

pub(crate) fn collect_units(
    input: DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
) -> Result<std::collections::BTreeMap<ParameterDefaultInfo, ParameterDefaultUnit>, ProjectionError>
{
    let mut units = std::collections::BTreeMap::new();
    for (&parameter, &expression) in &input.parameter_defaults {
        let reference = project(
            input,
            identities,
            &etas_types::ParameterDefaultRef::Source { parameter },
        )?;
        let Some(SymbolDef::Param { owner, .. }) =
            input.symbols.get(parameter).map(|symbol| &symbol.def)
        else {
            return Err(ProjectionError::Invalid(
                "default unit has no defining parameter".into(),
            ));
        };
        let scope = input
            .scopes
            .iter()
            .find(|scope| scope.symbols.contains(&parameter))
            .map(|scope| scope.id)
            .ok_or_else(|| {
                ProjectionError::Invalid("default unit has no definition scope".into())
            })?;
        let Some(etas_types::SymbolTypeFact::Param { ty }) =
            input.types.symbol_types.get(&parameter)
        else {
            return Err(ProjectionError::Invalid(
                "default unit has no declared output type".into(),
            ));
        };
        let unit = ParameterDefaultUnit {
            parameter,
            owner: *owner,
            expression,
            scope,
            output: *ty,
        };
        if units.insert(reference, unit).is_some() {
            return Err(ProjectionError::Invalid(
                "duplicate canonical default expression unit".into(),
            ));
        }
    }
    Ok(units)
}

/// Project the actual authored unit; session-local arena ids never leave this function.
pub(crate) fn project(
    input: DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    reference: &etas_types::ParameterDefaultRef,
) -> Result<ParameterDefaultInfo, ProjectionError> {
    let parameter = match reference {
        etas_types::ParameterDefaultRef::External { reference } => return Ok(reference.clone()),
        etas_types::ParameterDefaultRef::Source { parameter } => parameter,
    };
    let parameter = *parameter;
    let invalid = |message: &str| ProjectionError::Invalid(message.into());
    let SymbolDef::Param { owner, .. } = input
        .symbols
        .get(parameter)
        .ok_or_else(|| invalid("default parameter symbol is missing"))?
        .def
    else {
        return Err(invalid("default source is not a declared parameter"));
    };
    let key = |symbol: SymbolId| {
        identities
            .symbol(symbol)
            .cloned()
            .ok_or_else(|| invalid("default owner has no canonical declaration identity"))
    };
    let callable = |symbol: SymbolId| match identities.method(symbol) {
        Some(method) => Ok(ParameterDefaultOwner::Method(method.clone())),
        None => key(symbol).map(ParameterDefaultOwner::Declaration),
    };
    let owner = match &input.items[owner] {
        HirItem::Flow(flow) => callable(flow.symbol)?,
        HirItem::Agent(agent) => callable(agent.symbol)?,
        HirItem::Tool(tool) => callable(tool.symbol)?,
        HirItem::Spec(spec) => {
            let method = spec
                .items
                .iter()
                .find_map(|member| match member {
                    HirSpecItem::FlowSignature(signature)
                        if signature.params.contains(&parameter) =>
                    {
                        Some(signature)
                    }
                    _ => None,
                })
                .ok_or_else(|| invalid("default has no spec method signature"))?;
            ParameterDefaultOwner::SpecMethod {
                spec: key(spec.symbol)?,
                name: input
                    .symbols
                    .get(method.symbol)
                    .expect("checked spec method")
                    .name
                    .clone(),
            }
        }
        HirItem::Effect(effect) => {
            let action = effect
                .body
                .actions()
                .iter()
                .find(|action| action.params.contains(&parameter))
                .ok_or_else(|| invalid("default has no action declaration"))?;
            ParameterDefaultOwner::Action {
                effect: key(effect.symbol)?,
                name: input
                    .symbols
                    .get(action.symbol)
                    .expect("checked action")
                    .name
                    .clone(),
            }
        }
        HirItem::Impl(implementation) => {
            let HirImplTarget::Inherent { target, .. } = &implementation.target else {
                return Err(invalid("default action has no defining effect"));
            };
            let ResolveResult::Resolved(effect) = target.resolution else {
                return Err(invalid("default action effect is unresolved"));
            };
            let action = implementation
                .items
                .iter()
                .find_map(|member| match member {
                    HirImplItem::Action(action) if action.params.contains(&parameter) => {
                        Some(action)
                    }
                    _ => None,
                })
                .ok_or_else(|| invalid("default has no effect extension action"))?;
            ParameterDefaultOwner::Action {
                effect: key(effect)?,
                name: input
                    .symbols
                    .get(action.symbol)
                    .expect("checked action")
                    .name
                    .clone(),
            }
        }
        _ => return Err(invalid("default has no named callable declaration")),
    };
    owner
        .validate()
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
    let slot = input
        .parameter_list(parameter)
        .and_then(|parameters| parameters.iter().position(|symbol| *symbol == parameter))
        .ok_or_else(|| invalid("default parameter is absent from its declaration"))?
        as u32;
    let expr = *input
        .parameter_defaults
        .get(&parameter)
        .ok_or_else(|| invalid("default has no authored expression"))?;
    let span = input.exprs[expr].span(&input.blocks);
    let sources = input
        .sources
        .ok_or_else(|| invalid("default projection requires its defining source bundle"))?;
    let source = sources
        .sources
        .iter()
        .find(|source| source.id == span.source)
        .ok_or_else(|| invalid("default expression source is missing"))?;
    let text = source
        .text()
        .get(span.range.start.0 as usize..span.range.end.0 as usize)
        .ok_or_else(|| invalid("default expression source range is invalid"))?;
    let mut captures = capture_revisions(input, identities, expr, sources)?;
    captures.sort_unstable();
    captures.dedup();
    let owner_fingerprint = owner.fingerprint();
    let slot_bytes = slot.to_le_bytes();
    let mut parts = vec![
        owner_fingerprint.as_bytes().as_slice(),
        slot_bytes.as_slice(),
        text.as_bytes(),
    ];
    parts.extend(
        captures
            .iter()
            .map(|fingerprint| fingerprint.as_bytes().as_slice()),
    );
    Ok(ParameterDefaultInfo {
        injected: input.parameter_injections.contains(&parameter),
        owner,
        parameter: slot,
        revision: CodeFingerprint::of_parts("etas.code.parameter-default-source.v1", parts),
    })
}

fn capture_revisions(
    input: DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    root: etas_hir::HirExprId,
    sources: &crate::SourceBundle,
) -> Result<Vec<CodeFingerprint>, ProjectionError> {
    use etas_hir::{HirExpr, HirTreeChild};
    let mut pending = vec![HirTreeChild::Expr(root)];
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut revisions = Vec::new();
    while !pending.is_empty() || !items.is_empty() {
        if pending.is_empty() {
            let item = items.pop().expect("captured item");
            if !seen.insert(item) {
                continue;
            }
            let declaration = &input.items[item];
            if let Some(symbol) = declaration.symbol()
                && let Some(identity) = symbol_fingerprint(input, identities, symbol)
            {
                let span = declaration.span();
                let source = sources
                    .sources
                    .iter()
                    .find(|source| source.id == span.source)
                    .ok_or_else(|| {
                        ProjectionError::Invalid(
                            "captured default definition source is missing".into(),
                        )
                    })?;
                let text = source
                    .text()
                    .get(span.range.start.0 as usize..span.range.end.0 as usize)
                    .ok_or_else(|| {
                        ProjectionError::Invalid(
                            "captured default definition source range is invalid".into(),
                        )
                    })?;
                revisions.push(CodeFingerprint::of_parts(
                    "etas.code.default-capture-body.v1",
                    [identity.as_bytes().as_slice(), text.as_bytes()],
                ));
            }
            pending.extend(
                input
                    .hir
                    .index()
                    .expr_item
                    .iter()
                    .filter_map(|(expr, owner)| {
                        (*owner == item).then_some(HirTreeChild::Expr(*expr))
                    }),
            );
            continue;
        }
        match pending.pop().expect("default expression child") {
            HirTreeChild::Expr(expr) => {
                if let HirExpr::Path(path) = &input.exprs[expr]
                    && let ResolveResult::Resolved(symbol) = path.resolution
                {
                    let symbol = input.canonical_source_symbol(symbol).unwrap_or(symbol);
                    if let Some(identity) = symbol_fingerprint(input, identities, symbol) {
                        revisions.push(CodeFingerprint::of_parts(
                            "etas.code.default-capture-reference.v1",
                            std::iter::once(identity.as_bytes().as_slice())
                                .chain(path.segments.iter().map(|segment| segment.name.as_bytes())),
                        ));
                    }
                    if let Some(SymbolDef::Item { item }) =
                        input.symbols.get(symbol).map(|symbol| &symbol.def)
                    {
                        items.push(*item);
                    }
                }
                if let Some(method) = input.types.checked_method_calls.get(&expr) {
                    match &method.target {
                        etas_types::method::MethodTarget::Source {
                            symbol, callable, ..
                        }
                        | etas_types::method::MethodTarget::Agent { symbol, callable } => {
                            if let Some(identity) = symbol_fingerprint(input, identities, *symbol) {
                                revisions.push(identity);
                            }
                            items.push(*callable);
                        }
                        etas_types::method::MethodTarget::External { identity } => {
                            revisions.push(identity.fingerprint())
                        }
                        _ => {}
                    }
                }
                match &input.exprs[expr] {
                    HirExpr::Lambda {
                        body: etas_hir::HirLambdaBody::Expr(expr),
                        ..
                    } => pending.push(HirTreeChild::Expr(*expr)),
                    HirExpr::Lambda {
                        body: etas_hir::HirLambdaBody::Block(block),
                        ..
                    } => pending.push(HirTreeChild::Block(*block)),
                    _ => {
                        etas_hir::for_each_expr_child(input.hir, expr, |child| pending.push(child))
                    }
                }
            }
            HirTreeChild::Block(block) => {
                pending.extend(
                    input.blocks[block]
                        .stmts
                        .iter()
                        .copied()
                        .map(HirTreeChild::Stmt),
                );
                pending.extend(input.blocks[block].final_expr.map(HirTreeChild::Expr));
            }
            HirTreeChild::Stmt(stmt) => {
                etas_hir::for_each_stmt_child(input.hir, stmt, |child| pending.push(child))
            }
            HirTreeChild::HandlerArm(arm) => {
                pending.push(HirTreeChild::Block(input.handler_arms[arm].body))
            }
            HirTreeChild::Pattern(_) => {}
        }
    }
    Ok(revisions)
}

fn symbol_fingerprint(
    input: DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    symbol: SymbolId,
) -> Option<CodeFingerprint> {
    identities
        .method(symbol)
        .map(|method| method.fingerprint())
        .or_else(|| {
            identities
                .symbol(symbol)
                .or_else(|| input.types.external_definition_bindings.get(&symbol))
                .map(|key| key.fingerprint())
        })
}
