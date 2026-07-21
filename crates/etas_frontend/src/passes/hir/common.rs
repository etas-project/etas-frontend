use std::collections::HashMap;

use etas_utils::{PassContext, PassFailure, PassResult, PreservedArtifacts};

use crate::{MODULE_PART_UNIT_KIND, ModulePartId, ParsedSource, ProjectContext};

pub(super) fn current_module_part(
    pass_context: &PassContext<ProjectContext>,
) -> Result<ModulePartId, PassFailure> {
    let Some(unit) = pass_context.current_unit else {
        return Err(PassFailure::new(
            "module-part-scoped pass was invoked without a current unit",
        ));
    };
    if unit.kind != MODULE_PART_UNIT_KIND {
        return Err(PassFailure::new(format!(
            "module-part-scoped pass received mismatched unit kind {:?}",
            unit.kind
        )));
    }
    Ok(ModulePartId(unit.id as u32))
}

pub(super) fn failed_pass_result(failure: PassFailure) -> PassResult {
    PassResult {
        control: etas_utils::PassControl::Failed(failure),
        changed: false,
        preserved: PreservedArtifacts::All,
        produced: etas_utils::ArtifactSet::new(),
    }
}

pub(super) fn parsed_program_for_part(
    context: &ProjectContext,
    part_id: ModulePartId,
) -> etas_syntax::ast::Program {
    let part = context
        .modules
        .as_ref()
        .expect("module index should exist")
        .parts
        .get(part_id)
        .expect("module part should exist");
    let parsed_by_source = parsed_by_source(context);
    parsed_by_source
        .get(&part.source)
        .expect("module part source should be parsed")
        .parse
        .value
        .clone()
}

pub(super) fn parsed_by_source(
    context: &ProjectContext,
) -> HashMap<etas_core::SourceId, &ParsedSource> {
    context
        .parsed_sources
        .iter()
        .map(|parsed| (parsed.source, parsed))
        .collect()
}
