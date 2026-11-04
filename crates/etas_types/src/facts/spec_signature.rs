use crate::{
    CallableGenericParam, CallableGenericParamKind, CheckedSpecBound, CheckedSpecRef,
    SpecSignature, SymbolTypeFact, TypeFacts,
};

impl SpecSignature {
    pub(crate) fn generic_parameters_in(
        &self,
        facts: &TypeFacts,
        hir: &etas_hir::HirProgram,
    ) -> Option<Vec<CallableGenericParam>> {
        if self.params.len() != self.param_names.len() {
            return None;
        }
        self.params
            .iter()
            .zip(&self.param_names)
            .map(|(parameter, name)| {
                let declaration = hir.symbols.get(*parameter)?;
                let (kind, declared_bounds) = match &declaration.def {
                    etas_hir::SymbolDef::TypeParam { bounds, .. } => {
                        (CallableGenericParamKind::Type, bounds.len())
                    }
                    etas_hir::SymbolDef::EffectParam { .. } => {
                        (CallableGenericParamKind::Effect, 0)
                    }
                    _ => return None,
                };
                let bounds = facts
                    .type_param_bounds
                    .get(parameter)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if declaration.name != *name || declared_bounds != bounds.len() {
                    return None;
                }
                let SymbolTypeFact::Param { ty } = facts.symbol_types.get(parameter)? else {
                    return None;
                };
                Some(CallableGenericParam {
                    name: name.clone(),
                    kind,
                    subject: *ty,
                    bounds: bounds
                        .iter()
                        .map(|bound| CheckedSpecBound {
                            spec: CheckedSpecRef::Source(bound.spec_symbol),
                            args: bound.args.clone(),
                        })
                        .collect(),
                })
            })
            .collect()
    }
}
