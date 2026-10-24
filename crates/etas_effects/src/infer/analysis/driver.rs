use etas_hir_analysis::interprocedural::InterproceduralAnalysis;

use crate::{EffectPipelineError, EffectUnit, facts::EffectAnalysisOutput};

use super::{input::EffectAnalysisInput, output::output_from_result};
use crate::infer::semantics::engine::EffectSemantics;

pub fn run_effect_analysis(
    input: EffectAnalysisInput<'_>,
    units: &[EffectUnit],
) -> Result<EffectAnalysisOutput, EffectPipelineError> {
    let semantics = EffectSemantics::from_input(input);
    let result = InterproceduralAnalysis::new(units.iter().copied(), semantics).solve();
    output_from_result(result)
}
