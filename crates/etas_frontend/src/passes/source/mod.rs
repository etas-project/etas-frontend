use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassFailure, PassKind, PassManager, PassResult,
    PassScope, PreservedArtifacts,
};

use crate::{
    AstImportRef, AstItemKind, AstItemRef, ParsedSource, ProjectContext, SOURCE_FILE_UNIT_KIND,
    SourceFile, SourceSet,
};

use crate::passes::artifacts::{PARSED_SOURCE_SET, SOURCE_SET, unit_kind_with_diagnostics};

pub struct BuildSourceSetPass;

impl Pass<ProjectContext> for BuildSourceSetPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildSourceSetPass", PassKind::Transform)
            .produces(ArtifactSet::one(SOURCE_SET))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let files = context
            .input
            .sources
            .iter()
            .map(SourceFile::from_input)
            .collect::<Vec<_>>();
        context.sources = Some(SourceSet {
            project_root: context.input.project_root.clone(),
            source_root: crate::project::source_root_for_project_input(&context.input),
            environment_fingerprint: context
                .input
                .environment
                .canonical_environment_fingerprint(),
            external_modules_fingerprint: context
                .input
                .environment
                .canonical_external_modules_fingerprint(),
            files,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(SOURCE_SET))
    }
}

pub struct ParseSourceFilePass;

impl Pass<ProjectContext> for ParseSourceFilePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ParseSourceFilePass", PassKind::Transform)
            .scope(PassScope::Unit(SOURCE_FILE_UNIT_KIND))
            .requires(ArtifactSet::one(SOURCE_SET))
            .produces(unit_kind_with_diagnostics(
                PARSED_SOURCE_SET,
                SOURCE_FILE_UNIT_KIND,
            ))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let unit = match current_source_file_unit(pass_context) {
            Ok(unit) => unit,
            Err(failure) => return failed_pass_result(failure),
        };
        let Some(source_id) = context.source_file_for_unit(unit).map(|source| source.id) else {
            return failed_pass_result(PassFailure::new(format!(
                "source-file unit {:?} does not resolve to a source file",
                unit
            )));
        };
        if let Some(parsed) = context.parsed_source_reuse.remove(&source_id) {
            context
                .diagnostics
                .extend(parsed.diagnostics.iter().cloned());
            context.reused_parsed_sources += 1;
            context.parsed_sources.push(parsed);
            return PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::one(PARSED_SOURCE_SET),
            );
        }
        let Some(source_file) = context.source_file_for_unit(unit) else {
            return failed_pass_result(PassFailure::new(format!(
                "source-file unit {:?} does not resolve to a source file",
                unit
            )));
        };
        let source = source_file.to_core_source_file();
        let parsed = etas_syntax::parse_program(source.clone());
        let diagnostics = parsed.diagnostics.clone();
        context.diagnostics.extend(diagnostics.iter().cloned());

        let declared_module = parsed
            .value
            .module
            .as_ref()
            .map(|module| crate::ModulePath::from_ast(&module.path));
        let imports = parsed
            .value
            .imports
            .iter()
            .enumerate()
            .map(|(index, import)| AstImportRef {
                source: source.id,
                module_part: None,
                index,
                span: import.span,
            })
            .collect();
        let items = parsed
            .value
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| AstItemRef {
                source: source.id,
                module_part: None,
                index,
                kind: AstItemKind::from_item(&item.item),
                span: item.span(),
            })
            .collect();

        context.parsed_sources.push(ParsedSource {
            source: source.id,
            source_file: source,
            parse: parsed,
            declared_module,
            imports,
            items,
            diagnostics,
        });
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(PARSED_SOURCE_SET))
    }
}

fn current_source_file_unit(
    pass_context: &PassContext<ProjectContext>,
) -> Result<etas_utils::UnitKey, PassFailure> {
    let Some(unit) = pass_context.current_unit else {
        return Err(PassFailure::new(
            "source-file-scoped pass was invoked without a current unit",
        ));
    };
    if unit.kind != SOURCE_FILE_UNIT_KIND {
        return Err(PassFailure::new(format!(
            "source-file-scoped pass received mismatched unit kind {:?}",
            unit.kind
        )));
    }
    Ok(unit)
}

fn failed_pass_result(failure: PassFailure) -> PassResult {
    PassResult {
        control: etas_utils::PassControl::Failed(failure),
        changed: false,
        preserved: PreservedArtifacts::All,
        produced: ArtifactSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use etas_core::SourceId;
    use etas_utils::{Pass, PassContext, PassControl, PassManager, UnitKey};

    use super::{BuildSourceSetPass, ParseSourceFilePass};
    use crate::{BODY_UNIT_KIND, ProjectContext, ProjectInput, SourceInput, SourceKind};

    #[test]
    fn parse_source_file_pass_reports_structured_failure_for_wrong_unit_kind() {
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput {
            id: SourceId(999),
            path: None,
            text: "flow main() -> unit { return; }".to_owned(),
            kind: SourceKind::SingleFileInput,
        }));
        let mut build_sources = BuildSourceSetPass;
        let mut parse_source = ParseSourceFilePass;
        let mut manager = PassManager::new();

        let build_result = build_sources.run(&mut context, &PassContext::new(None), &mut manager);
        assert!(matches!(build_result.control, PassControl::Continue));

        let result = parse_source.run(
            &mut context,
            &PassContext::new(Some(UnitKey::new(BODY_UNIT_KIND, 0))),
            &mut manager,
        );

        let PassControl::Failed(failure) = result.control else {
            panic!("expected structured failure for mismatched source-file unit");
        };
        assert!(failure.message.contains("source-file-scoped pass"));
        assert!(failure.message.contains("mismatched unit kind"));
    }
}
