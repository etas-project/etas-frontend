use std::path::{Path, PathBuf};

use etas_frontend::{Frontend, OptimizationLevel, ProjectInput, SourceInput};

#[test]
fn every_semantics_preserving_optimization_fixture_compiles_at_all_levels() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/optimization");
    let mut fixtures = source_fixtures(&root);
    fixtures.retain(|fixture| {
        !fixture
            .components()
            .any(|component| component.as_os_str() == "quality_aware_context_policy_budget")
    });
    assert!(
        fixtures.len() >= 120,
        "found only {} fixtures",
        fixtures.len()
    );

    for fixture in fixtures {
        let source = std::fs::read_to_string(&fixture).unwrap();
        for level in [
            OptimizationLevel::O0,
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ] {
            let mut input = ProjectInput::single_source(SourceInput::anonymous(source.clone()));
            input.options.optimization_level = level;
            let output = Frontend.check_project(input);
            assert!(
                output.diagnostics.is_empty(),
                "{} {level}: {:?}",
                fixture.display(),
                output.diagnostics
            );
            let checked = output.checked.expect("optimization fixture should check");
            assert_eq!(checked.compile_options.optimization_level, level);
            assert_eq!(checked.optimization_report.requested_level, level);
            assert_eq!(checked.optimization_report.effective_level, level);
            checked
                .hir
                .clone()
                .edit()
                .finish()
                .unwrap_or_else(|error| panic!("{} {level}: {error:?}", fixture.display()));
        }
    }
}

#[test]
fn context_budget_policy_is_independent_from_optimization_level() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/optimization/quality_aware_context_policy_budget");
    let fixtures = source_fixtures(&root);
    assert_eq!(fixtures.len(), 10);
    for fixture in fixtures {
        let source = std::fs::read_to_string(&fixture).unwrap();
        for level in [
            OptimizationLevel::O0,
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ] {
            let mut input = ProjectInput::single_source(SourceInput::anonymous(source.clone()));
            input.options.optimization_level = level;
            input.options.quality_context_token_budget = Some(8);
            let output = Frontend.check_project(input);
            assert!(
                output.diagnostics.is_empty(),
                "{} {level}: {:?}",
                fixture.display(),
                output.diagnostics
            );
            let checked = output.checked.expect("policy fixture should check");
            assert!(
                checked
                    .optimization_report
                    .policy_transform("ApplyQualityContextBudgetPolicy")
                    .is_some(),
                "{} {level}",
                fixture.display()
            );
        }
    }
}

fn source_fixtures(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut fixtures = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "es") {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();
    fixtures
}
