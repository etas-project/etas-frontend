use super::overlay::CANDIDATE_BODY_OVERLAYS_APPLIED;
use crate::{
    ModulePath, ProjectContext, ProjectEntry, ProjectEntryFact, ProjectEntryResolution,
    passes::artifacts::{HIR_OUTPUT, PROJECT_ANALYSIS_ROOTS, PROJECT_ENTRY, RESOLVED_PATHS},
};
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

pub(crate) const CANDIDATE_TARGET: ArtifactKey = ArtifactKey::new("code", "candidate_target");
pub(crate) struct ResolveCandidateTargetPass;

impl Pass<ProjectContext> for ResolveCandidateTargetPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ResolveCandidateTargetPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                RESOLVED_PATHS,
                CANDIDATE_BODY_OVERLAYS_APPLIED,
            ]))
            .produces(ArtifactSet::from([
                CANDIDATE_TARGET,
                PROJECT_ENTRY,
                PROJECT_ANALYSIS_ROOTS,
            ]))
    }
    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(candidate), Some(hir), Some(bindings)) = (
            &mut context.candidate,
            &context.hir,
            &context.hir_body_bindings,
        ) else {
            return PassResult::failed(
                "candidate target resolution requires its body bindings and finalized HIR",
            );
        };
        let slot = candidate.overlay.slot();
        let targets = bindings
            .body_items
            .iter()
            .filter(|(body, _)| {
                body.source == slot.source
                    && body.item.index == slot.item.index
                    && body.impl_member == slot.impl_member
            })
            .map(|(_, item)| *item)
            .collect::<Vec<_>>();
        let [target] = targets.as_slice() else {
            return PassResult::failed(
                "candidate default body does not have exactly one authoritative HIR owner",
            );
        };
        let Some(decl) = hir.hir.items.get(*target) else {
            return PassResult::failed("candidate default body HIR item is missing");
        };
        if decl.span() != candidate.overlay.declaration() {
            return PassResult::failed("candidate body changed its declaration header identity");
        }
        let Some(symbol) = decl.symbol().and_then(|symbol| hir.hir.symbols.get(symbol)) else {
            return PassResult::failed("candidate body has no resolved declaration symbol");
        };
        let Some(module) = hir.hir.modules_arena.get(symbol.defining_module) else {
            return PassResult::failed("candidate body has no definition module");
        };
        candidate.target = Some(*target);
        let mut roots = std::collections::BTreeSet::from([*target]);
        for slot in candidate.overlay.frozen().callable_slots().values() {
            let owners = bindings
                .body_items
                .iter()
                .filter(|(body, _)| {
                    body.source == slot.source
                        && body.item.index == slot.item.index
                        && body.impl_member == slot.impl_member
                })
                .map(|(_, item)| *item)
                .collect::<Vec<_>>();
            let [owner] = owners.as_slice() else {
                return PassResult::failed(
                    "frozen callable does not have exactly one candidate body owner",
                );
            };
            if hir.hir.items.get(*owner).is_none() {
                return PassResult::failed("frozen callable candidate body owner is missing");
            }
            roots.insert(*owner);
        }
        context.analysis_roots = roots.into_iter().collect();
        context.entry = Some(ProjectEntryFact {
            requested: ProjectEntry {
                module: module.name.as_ref().map(|name| ModulePath {
                    segments: name
                        .segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect(),
                }),
                flow: symbol.name.clone(),
            },
            resolved: Some(ProjectEntryResolution {
                module: symbol.defining_module,
                item: *target,
            }),
        });
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from([CANDIDATE_TARGET, PROJECT_ENTRY, PROJECT_ANALYSIS_ROOTS]),
        )
    }
}
