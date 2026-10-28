use super::{engine::EffectSemantics, shared::*};

impl EffectSemantics<'_> {
    pub(super) fn record_std_action_call(
        &mut self,
        call: HirExprId,
        intrinsic: Option<u32>,
        summary: &EffectSummary,
    ) -> Result<(), String> {
        let intrinsic = intrinsic.ok_or("standard action adapter has no intrinsic identity")?;
        let mut requested = summary.requested_actions.effects.iter();
        let invocation = requested
            .next()
            .filter(|effect| effect.is_action())
            .ok_or("standard action adapter has no checked invocation")?
            .clone();
        if requested.next().is_some() || summary.requested_actions.open.is_some() {
            return Err("standard action adapter must have one closed checked invocation".into());
        }
        let ty = *self
            .types
            .facts
            .expr_types
            .get(&call)
            .ok_or("standard action adapter has no result type")?;
        let (parameters, output) = match self.hir.exprs.get(call) {
            Some(HirExpr::Call { args, .. }) => {
                let parameters = args
                    .iter()
                    .map(|arg| {
                        arg_expr(arg)
                            .and_then(|expr| self.types.facts.expr_types.get(&expr).copied())
                            .ok_or("standard action argument has no checked type")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (parameters, ty)
            }
            Some(HirExpr::Path(_)) => match self.types.store.get(ty) {
                Some(Type::Function(flow)) => (flow.input.clone(), flow.output),
                _ => return Err("standard action reference has no checked function type".into()),
            },
            _ => return Err("standard action adapter has no checked callable expression".into()),
        };
        self.inputs.std_action_calls.insert(
            call,
            crate::facts::StdActionCallFact {
                intrinsic,
                parameters,
                output,
                invocation,
            },
        );
        Ok(())
    }
}
