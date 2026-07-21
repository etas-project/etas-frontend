use etas_hir::{HirExprId, HirHandlerArmId};

use crate::intraprocedural::{AbstractDomain, Control};

use super::{AnalysisUnit, CallSite, CallTarget, HirAnalysisBody, UnitContext};

pub trait InterproceduralSemantics {
    type Unit: AnalysisUnit;
    type Domain: AbstractDomain;
    type Summary: AbstractDomain;

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody;

    fn begin_unit(&mut self, context: UnitContext<Self::Unit>) -> Self::Domain;

    fn end_unit(
        &mut self,
        context: UnitContext<Self::Unit>,
        exit: Control<Self::Domain>,
    ) -> Self::Summary;

    fn stabilize_summary(
        &mut self,
        _context: UnitContext<Self::Unit>,
        summary: Self::Summary,
        _current: Option<&Self::Summary>,
    ) -> Self::Summary {
        summary
    }

    fn external_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary;

    fn missing_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary;

    fn call_target(
        &mut self,
        context: UnitContext<Self::Unit>,
        call: HirExprId,
        callee: HirExprId,
        state: &Self::Domain,
    ) -> CallTarget<Self::Unit>;

    fn handler_arm_dependency(
        &mut self,
        _context: UnitContext<Self::Unit>,
        _arm: HirHandlerArmId,
    ) -> Option<Self::Unit> {
        None
    }

    fn anonymous_flow_dependency(
        &mut self,
        _context: UnitContext<Self::Unit>,
        _expr: HirExprId,
    ) -> Option<Self::Unit> {
        None
    }

    fn direct_call(
        &mut self,
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        callee: Self::Unit,
        summary: &Self::Summary,
        state: Self::Domain,
    ) -> Self::Domain;

    fn dynamic_call(
        &mut self,
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain;

    fn external_call(
        &mut self,
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain;

    fn incomplete_call(
        &mut self,
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain;
}
