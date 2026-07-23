use etas_hir_analysis::HirAnalysisContext;
use etas_hir_analysis::interprocedural::InterproceduralAnalysis;

use crate::{EffectPipelineError, EffectUnit, facts::EffectAnalysisOutput};

use super::{input::EffectAnalysisInput, output::output_from_result};
use crate::infer::semantics::engine::EffectSemantics;

pub fn run_effect_analysis(
    input: EffectAnalysisInput<'_>,
    units: &[EffectUnit],
    context: HirAnalysisContext,
) -> Result<EffectAnalysisOutput, EffectPipelineError> {
    let semantics = EffectSemantics::with_context(
        input.hir,
        context,
        input.types,
        input.std_registry,
        input.registry,
        input.tool_bindings,
        input.external_summaries,
    );
    let result = InterproceduralAnalysis::new(units.iter().copied(), semantics).solve();
    output_from_result(result)
}
