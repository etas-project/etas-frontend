use etas_hir::HirExprId;

use super::state::EffectState;
use crate::infer::unit::{EffectAnonymousFlowBody, EffectUnit};

impl EffectState {
    pub fn record_anonymous_flow(
        &mut self,
        owner: etas_hir::HirItemId,
        value: HirExprId,
        body: EffectAnonymousFlowBody,
    ) {
        self.local_latent_values
            .record_value(value, EffectUnit::AnonymousFlow { owner, value, body });
    }
}
