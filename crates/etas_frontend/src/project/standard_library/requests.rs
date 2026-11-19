use std::collections::BTreeMap;

use etas_hir::HirExpr;
use etas_types::{TypeId, TypeSubstitutionEngine};

use crate::CheckedProgramData;

use super::super::instantiation;

pub(super) struct ControlTypes {
    pub requests: Vec<TypeId>,
    pub values: Vec<TypeId>,
}

/// The caller owns its Perform results, handled body and lexical exits. Prepare
/// their native types from actual checked facts and generic instances.
pub(super) fn collect(program: &CheckedProgramData) -> Result<ControlTypes, String> {
    let mut instances = BTreeMap::<_, Vec<_>>::new();
    for instance in program.native_instantiations.instances() {
        instances.entry(instance.owner).or_default().push(instance);
    }
    let index = program.hir.index();
    let limits = etas_code::descriptor::DescriptorLimits::default();
    let control = super::Continue;
    let mut result = ControlTypes {
        requests: Vec::new(),
        values: Vec::new(),
    };
    for (site, output) in &program.types.expr_types {
        if program.hir.program().is_expr_optimized_out(*site) {
            continue;
        }
        let roots = match program.hir.exprs.get(*site) {
            Some(HirExpr::Perform { .. }) => vec![(true, *output)],
            Some(HirExpr::Handle { body, .. }) => {
                let exits = program.lexical_exit_types(*site)?;
                let mut roots = vec![(false, *output), (false, program.types.expr_types[body])];
                roots.extend(
                    [exits.returning, exits.resuming, exits.finishing]
                        .into_iter()
                        .flatten()
                        .map(|ty| (false, ty)),
                );
                roots
            }
            _ => continue,
        };
        let owner = *index
            .expr_item
            .get(site)
            .ok_or("caller request has no checked body owner")?;
        let signature = instantiation::signature(&program.types, owner);
        let mut push = |request, ty| {
            result.values.push(ty);
            if request {
                result.requests.push(ty);
            }
        };
        for (request, output) in roots {
            if signature.is_none_or(|signature| signature.generic_params.is_empty())
                && instantiation::owner_parameters(&program.hir, &program.types, owner).is_empty()
            {
                push(request, output);
            }
            for instance in instances.get(&owner).into_iter().flatten() {
                let (types, rows) = instance.bindings(signature)?;
                let parameters = instance.type_parameters(signature);
                let mut substitution =
                    TypeSubstitutionEngine::existing(&program.type_store, &types)
                        .with_type_parameters(&parameters)
                        .with_effect_arguments(&rows)
                        .with_expansion_depth(limits.max_expansion_depth)
                        .with_execution_control(Some(&control));
                push(
                    request,
                    substitution.substitute(output).map_err(|e| e.to_string())?,
                );
            }
        }
    }
    result.requests.sort_unstable();
    result.requests.dedup();
    result.values.sort_unstable();
    result.values.dedup();
    Ok(result)
}
