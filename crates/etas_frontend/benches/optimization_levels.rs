use std::{path::Path, time::Instant};

use etas_core::memory::MemoryFootprint;
use etas_frontend::{Frontend, OptimizationLevel, ProjectInput, SourceInput};

fn main() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/optimization");
    let fixtures = source_fixtures(&fixture_root)
        .into_iter()
        .filter(|fixture| {
            !fixture
                .components()
                .any(|component| component.as_os_str() == "quality_aware_context_policy_budget")
        })
        .collect::<Vec<_>>();

    println!(
        "level,fixtures,compile_ms,retained_bytes,hir_nodes,optimized_out_nodes,rewritten_nodes"
    );
    for level in [
        OptimizationLevel::O0,
        OptimizationLevel::O1,
        OptimizationLevel::O2,
        OptimizationLevel::O3,
    ] {
        let started = Instant::now();
        let mut retained_bytes = 0_u64;
        let mut hir_nodes = 0_usize;
        let mut optimized_out_nodes = 0_usize;
        let mut rewritten_nodes = 0_usize;

        for fixture in &fixtures {
            let source = std::fs::read_to_string(fixture).unwrap();
            let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
            input.options.optimization_level = level;
            let output = Frontend.check_project(input);
            assert!(
                output.diagnostics.is_empty(),
                "{} {level}: {:?}",
                fixture.display(),
                output.diagnostics
            );
            let checked = output.checked.unwrap();
            retained_bytes =
                retained_bytes.saturating_add(MemoryFootprint::of(&checked).estimated_bytes());
            let program = checked.hir.program();
            hir_nodes += program.items.len()
                + program.exprs.len()
                + program.stmts.len()
                + program.pats.len()
                + program.blocks.len();
            optimized_out_nodes += program
                .exprs
                .iter()
                .filter(|(id, _)| program.is_expr_optimized_out(*id))
                .count()
                + program
                    .stmts
                    .iter()
                    .filter(|(id, _)| program.is_statement_optimized_out(*id))
                    .count()
                + program
                    .pats
                    .iter()
                    .filter(|(id, _)| program.is_pattern_optimized_out(*id))
                    .count()
                + program
                    .blocks
                    .iter()
                    .filter(|(id, _)| program.is_block_optimized_out(*id))
                    .count();
            rewritten_nodes += checked
                .optimization_report
                .passes
                .iter()
                .map(|pass| pass.rewritten)
                .sum::<usize>();
        }

        println!(
            "{level},{},{},{retained_bytes},{hir_nodes},{optimized_out_nodes},{rewritten_nodes}",
            fixtures.len(),
            started.elapsed().as_millis(),
        );
    }
}

fn source_fixtures(root: &Path) -> Vec<std::path::PathBuf> {
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
