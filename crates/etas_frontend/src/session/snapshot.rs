use std::collections::HashMap;

use etas_cache::{ArtifactFingerprint, ProjectRevision};
use etas_core::{SourceFile as CoreSourceFile, SourceId, Span};
use etas_effects::{EffectFacts, InterpreterSupportFacts};
use etas_hir::{
    HirBlockId, HirEffectArg, HirExpr, HirExprId, HirItem, HirItemId, HirNodeRef, HirOrigin,
    HirPat, HirPatId, HirProgram, HirStmt, HirType, HirTypeId, ResolveResult, ScopeTree, SourceMap,
    SymbolDef, SymbolId, SymbolTable, SyntaxNodeId, SyntaxNodeRef,
};
use etas_types::{EffectArgRef, ItemSignature, Type, TypeFacts, TypeId, TypeStore};

use crate::artifact::{
    FrontendArtifactDependency, FrontendArtifactKey, FrontendArtifactKind,
    FrontendArtifactManifest, FrontendArtifactRecord, fingerprint_text,
};
use crate::incremental::{BodyArtifactIdentity, DiagnosticSet};
use crate::{
    BODY_UNIT_KIND, HirPathResolution, ImportGraph, ImportTarget, ModuleIndex, ProjectEntryFact,
    ProjectOutput, ResolvedImports, ResolvedModuleTarget, ResolvedPaths, SourceSet, UnitId,
    UnitKind, UnitTarget, UnitTree,
};

struct BodyFactDependencyInputs<'a> {
    common: &'a [FrontendArtifactKey],
    body_hir: &'a [FrontendArtifactKey],
    item_signatures: &'a [FrontendArtifactKey],
}

#[derive(Clone, Debug)]
pub struct ProjectSemanticSnapshot {
    pub revision: ProjectRevision,
    pub sources: SourceSet,
    pub modules: ModuleIndex,
    pub units: UnitTree,
    pub import_graph: ImportGraph,
    pub resolved_imports: ResolvedImports,
    pub resolved_paths: ResolvedPaths,
    pub hir: HirProgram,
    pub source_map: SourceMap,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub types: TypeFacts,
    pub type_store: TypeStore,
    pub effects: EffectFacts,
    pub interpreter_support: InterpreterSupportFacts,
    pub entry_fact: ProjectEntryFact,
    pub entry: Option<HirItemId>,
    pub definition_targets: SnapshotDefinitionTargets,
    pub diagnostics: DiagnosticSet,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotDefinitionTargets {
    pub symbols: HashMap<SymbolId, DefinitionTarget>,
    pub exprs: HashMap<HirExprId, DefinitionTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionTarget {
    SourceSymbol {
        symbol: SymbolId,
        item: Option<HirItemId>,
        span: Span,
    },
    Import(ImportTarget),
}

impl ProjectSemanticSnapshot {
    pub(crate) fn from_output(revision: ProjectRevision, output: &ProjectOutput) -> Option<Self> {
        let checked = output.checked.as_ref()?;
        let resolved_paths = output.resolved_paths.clone()?;
        let definition_targets =
            SnapshotDefinitionTargets::from_parts(&checked.symbols, &resolved_paths);
        Some(Self {
            revision,
            sources: output.sources.clone()?,
            modules: output.modules.clone()?,
            units: output.units.clone()?,
            import_graph: output.import_graph.clone()?,
            resolved_imports: output.resolved_imports.clone()?,
            resolved_paths,
            hir: checked.hir.clone(),
            source_map: checked.source_map.clone(),
            symbols: checked.symbols.clone(),
            scopes: checked.scopes.clone(),
            types: checked.types.clone(),
            type_store: checked.type_store.clone(),
            effects: checked.effects.clone(),
            interpreter_support: checked.interpreter_support.clone(),
            entry_fact: checked.entry_fact.clone(),
            entry: checked.entry,
            definition_targets,
            diagnostics: DiagnosticSet {
                diagnostics: output.diagnostics.clone(),
            },
        })
    }

    pub fn definition_target_for_symbol(&self, symbol: SymbolId) -> Option<&DefinitionTarget> {
        self.definition_targets.symbols.get(&symbol)
    }

    pub fn definition_target_for_expr(&self, expr: HirExprId) -> Option<&DefinitionTarget> {
        self.definition_targets.exprs.get(&expr)
    }

    pub fn syntax_node(&self, syntax: SyntaxNodeId) -> Option<&SyntaxNodeRef> {
        self.source_map.syntax_nodes.get(&syntax)
    }

    pub fn hir_nodes_for_syntax(&self, syntax: SyntaxNodeId) -> &[HirNodeRef] {
        self.source_map
            .syntax_to_hir
            .get(&syntax)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn hir_origin(&self, node: HirNodeRef) -> Option<&HirOrigin> {
        match node {
            HirNodeRef::Item(id) => self.source_map.item_sources.get(&id),
            HirNodeRef::Expr(id) => self.source_map.expr_sources.get(&id),
            HirNodeRef::HandlerArm(id) => self.source_map.handler_arm_sources.get(&id),
            HirNodeRef::Stmt(id) => self.source_map.stmt_sources.get(&id),
            HirNodeRef::Pat(id) => self.source_map.pat_sources.get(&id),
            HirNodeRef::Type(id) => self.source_map.type_sources.get(&id),
            HirNodeRef::Block(id) => self.source_map.block_sources.get(&id),
            HirNodeRef::Symbol(id) => self.source_map.symbol_sources.get(&id),
        }
    }
}

impl SnapshotDefinitionTargets {
    fn from_parts(symbols: &SymbolTable, resolved_paths: &ResolvedPaths) -> Self {
        let symbol_targets = symbols
            .iter()
            .map(|symbol| {
                (
                    symbol.id,
                    DefinitionTarget::SourceSymbol {
                        symbol: symbol.id,
                        item: symbol.defining_item,
                        span: symbol.definition_span,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let expr_targets = resolved_paths
            .expr_paths
            .iter()
            .filter_map(|path| {
                definition_target_for_path_resolution(&path.result, &symbol_targets)
                    .map(|target| (path.expr, target))
            })
            .collect::<HashMap<_, _>>();
        Self {
            symbols: symbol_targets,
            exprs: expr_targets,
        }
    }
}

fn definition_target_for_path_resolution(
    resolution: &HirPathResolution,
    symbols: &HashMap<SymbolId, DefinitionTarget>,
) -> Option<DefinitionTarget> {
    match resolution {
        HirPathResolution::DirectSymbol(symbol) => symbols.get(symbol).cloned(),
        HirPathResolution::ExplicitImport { target } => {
            Some(DefinitionTarget::Import(target.clone()))
        }
        HirPathResolution::PartialImport { target, .. } => {
            Some(DefinitionTarget::Import(target.clone()))
        }
        HirPathResolution::WildcardImport { target } => Some(DefinitionTarget::Import(
            ImportTarget::Module(target.clone()),
        )),
        HirPathResolution::PartialSymbol { prefix, .. } => symbols.get(prefix).cloned(),
        HirPathResolution::AmbiguousWildcard { .. }
        | HirPathResolution::PartiallyResolved { .. }
        | HirPathResolution::Unresolved => None,
    }
}

impl FrontendArtifactManifest {
    pub(crate) fn from_output(revision: ProjectRevision, output: &ProjectOutput) -> Self {
        let mut manifest = Self::new();
        for parsed in &output.parsed_sources {
            let key =
                FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, parsed.source);
            manifest.record(FrontendArtifactRecord {
                fingerprint: source_fingerprint(&parsed.source_file),
                key,
                revision,
                dependencies: Vec::new(),
                diagnostics: vec![parsed.source],
            });
        }

        if let Some(sources) = &output.sources {
            let key = FrontendArtifactKey::project(FrontendArtifactKind::SourceSet);
            manifest.record(FrontendArtifactRecord {
                fingerprint: source_set_fingerprint(sources),
                key,
                revision,
                dependencies: Vec::new(),
                diagnostics: Vec::new(),
            });
        }

        let parsed_sources = output
            .parsed_sources
            .iter()
            .map(|parsed| {
                FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, parsed.source)
            })
            .collect::<Vec<_>>();
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ParsedSourceSet,
            !output.parsed_sources.is_empty(),
            parsed_sources.clone(),
        );

        let source_set = FrontendArtifactKey::project(FrontendArtifactKind::SourceSet);
        let parsed_source_set = FrontendArtifactKey::project(FrontendArtifactKind::ParsedSourceSet);
        let module_index = FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex);
        let unit_tree = FrontendArtifactKey::project(FrontendArtifactKind::UnitTree);
        let import_graph = FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph);
        let module_topo_order = FrontendArtifactKey::project(FrontendArtifactKind::ModuleTopoOrder);
        let affected_module_set =
            FrontendArtifactKey::project(FrontendArtifactKind::AffectedModuleSet);
        let hir_program = FrontendArtifactKey::project(FrontendArtifactKind::HirProgram);
        let resolved_imports = FrontendArtifactKey::project(FrontendArtifactKind::ResolvedImports);
        let resolved_paths = FrontendArtifactKey::project(FrontendArtifactKind::ResolvedPaths);
        let signature_facts = FrontendArtifactKey::project(FrontendArtifactKind::SignatureFacts);
        let top_level_lets = FrontendArtifactKey::project(FrontendArtifactKind::TopLevelLetFacts);
        let type_facts = FrontendArtifactKey::project(FrontendArtifactKind::TypeFacts);
        let effect_facts = FrontendArtifactKey::project(FrontendArtifactKind::EffectFacts);
        let project_entry = FrontendArtifactKey::project(FrontendArtifactKind::ProjectEntry);
        let reachability = FrontendArtifactKey::project(FrontendArtifactKind::ReachabilityFacts);

        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ModuleIndex,
            output.modules.is_some(),
            vec![source_set.clone(), parsed_source_set.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::UnitTree,
            output.units.is_some(),
            vec![module_index.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ImportGraph,
            output.import_graph.is_some(),
            vec![module_index.clone(), parsed_source_set.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ModuleTopoOrder,
            output.module_topo_order.is_some(),
            vec![import_graph.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::AffectedModuleSet,
            output.affected_modules.is_some(),
            vec![import_graph.clone(), unit_tree.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::HirProgram,
            output.hir.is_some(),
            vec![
                module_index.clone(),
                unit_tree.clone(),
                parsed_source_set.clone(),
            ],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ResolvedImports,
            output.resolved_imports.is_some(),
            vec![
                import_graph.clone(),
                module_topo_order.clone(),
                hir_program.clone(),
            ],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ResolvedPaths,
            output.resolved_paths.is_some(),
            vec![resolved_imports.clone(), hir_program.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::SignatureFacts,
            output.signature_types.is_some(),
            vec![resolved_paths.clone(), hir_program.clone()],
        );
        let item_signature_facts =
            manifest.record_item_signature_facts(revision, output, signature_facts.clone());
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::TopLevelLetFacts,
            output.top_level_lets.is_some(),
            vec![
                signature_facts.clone(),
                resolved_paths.clone(),
                hir_program.clone(),
            ],
        );
        let body_hir = manifest.record_body_hir_artifacts(revision, output);
        let body_type_facts = manifest.record_body_facts(
            revision,
            FrontendArtifactKind::TypeFacts,
            output.type_body_outputs.keys().copied().collect(),
            BodyFactDependencyInputs {
                common: &[],
                body_hir: &body_hir,
                item_signatures: &item_signature_facts,
            },
            output,
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::TypeFacts,
            output.types.is_some(),
            vec![signature_facts.clone(), top_level_lets.clone()]
                .into_iter()
                .chain(body_type_facts.clone())
                .collect(),
        );
        manifest.record_project_if_present_with_parts(
            revision,
            FrontendArtifactKind::EffectPipelineArtifacts,
            output.effect_pipeline_artifacts.is_some(),
            vec![
                resolved_imports.clone(),
                resolved_paths.clone(),
                hir_program.clone(),
                signature_facts.clone(),
                type_facts.clone(),
            ],
            effect_pipeline_fingerprint_parts(output),
        );
        let effect_pipeline_artifacts =
            FrontendArtifactKey::project(FrontendArtifactKind::EffectPipelineArtifacts);
        let body_effect_facts = manifest.record_body_effect_facts(
            revision,
            output.effect_body_outputs.keys().copied().collect(),
            body_hir,
            output,
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::EffectFacts,
            output.effects.is_some(),
            vec![effect_pipeline_artifacts]
                .into_iter()
                .chain(body_effect_facts)
                .collect(),
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ProjectEntry,
            output.entry.is_some(),
            vec![module_index.clone(), hir_program.clone()],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::ReachabilityFacts,
            output.reachability.is_some(),
            vec![
                project_entry.clone(),
                resolved_paths.clone(),
                hir_program.clone(),
                unit_tree.clone(),
            ],
        );
        manifest.record_project_if_present(
            revision,
            FrontendArtifactKind::CheckedProject,
            output.checked.is_some(),
            vec![
                source_set.clone(),
                module_index.clone(),
                unit_tree.clone(),
                import_graph.clone(),
                affected_module_set,
                resolved_imports,
                resolved_paths,
                hir_program,
                top_level_lets,
                type_facts,
                effect_facts,
                project_entry,
                reachability,
            ],
        );

        for source in diagnostic_sources(output) {
            let mut dependencies = output
                .parsed_sources
                .iter()
                .find(|parsed| parsed.source == source)
                .map(|parsed| {
                    vec![FrontendArtifactKey::source(
                        FrontendArtifactKind::ParsedSource,
                        parsed.source,
                    )]
                })
                .unwrap_or_default();
            dependencies.push(FrontendArtifactKey::project(
                FrontendArtifactKind::CheckedProject,
            ));
            dependencies.retain(|key| manifest.get(key).is_some());
            manifest.record_source_with_dependencies(
                revision,
                FrontendArtifactKind::Diagnostics,
                source,
                dependencies,
            );
        }

        manifest
    }

    fn record_project_if_present(
        &mut self,
        revision: ProjectRevision,
        kind: FrontendArtifactKind,
        present: bool,
        dependencies: Vec<FrontendArtifactKey>,
    ) {
        self.record_project_if_present_with_parts(
            revision,
            kind,
            present,
            dependencies,
            Vec::new(),
        );
    }

    fn record_project_if_present_with_parts(
        &mut self,
        revision: ProjectRevision,
        kind: FrontendArtifactKind,
        present: bool,
        dependencies: Vec<FrontendArtifactKey>,
        extra_parts: Vec<String>,
    ) {
        if !present {
            return;
        }
        let dependencies = dependencies
            .into_iter()
            .filter_map(|dependency| self.dependency_record(dependency))
            .collect::<Vec<_>>();
        let key = FrontendArtifactKey::project(kind);
        self.record(FrontendArtifactRecord {
            fingerprint: self.fingerprint_for_with_parts(kind, &dependencies, extra_parts),
            key,
            revision,
            dependencies,
            diagnostics: Vec::new(),
        });
    }

    fn record_source_with_dependencies(
        &mut self,
        revision: ProjectRevision,
        kind: FrontendArtifactKind,
        source: SourceId,
        dependencies: Vec<FrontendArtifactKey>,
    ) {
        let key = FrontendArtifactKey::source(kind, source);
        let dependencies = dependencies
            .into_iter()
            .filter_map(|dependency| self.dependency_record(dependency))
            .collect::<Vec<_>>();
        self.record(FrontendArtifactRecord {
            fingerprint: self.fingerprint_for(kind, &dependencies),
            key,
            revision,
            dependencies,
            diagnostics: vec![source],
        });
    }

    fn record_body_facts(
        &mut self,
        revision: ProjectRevision,
        kind: FrontendArtifactKind,
        mut units: Vec<UnitId>,
        dependency_inputs: BodyFactDependencyInputs<'_>,
        output: &ProjectOutput,
    ) -> Vec<FrontendArtifactKey> {
        units.sort_by_key(|unit| unit.0);
        units
            .into_iter()
            .map(|unit| {
                let key = FrontendArtifactKey::unit(
                    kind,
                    etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
                );
                let body_signature_dependencies =
                    body_signature_dependencies(output, unit, dependency_inputs.item_signatures);
                let dependencies = dependency_inputs
                    .common
                    .iter()
                    .chain(dependency_inputs.body_hir.iter().filter(|body_key| {
                        matches!(
                            body_key.unit,
                            crate::artifact::FrontendUnitKey::Unit(unit_key)
                                if unit_key.id == unit.0 as u64
                        )
                    }))
                    .chain(body_signature_dependencies.iter())
                    .filter_map(|dependency| self.dependency_record(dependency.clone()))
                    .collect::<Vec<_>>();
                let body_parts = body_fingerprint_parts(output, unit);
                self.record(FrontendArtifactRecord {
                    fingerprint: self.fingerprint_for_with_parts(kind, &dependencies, body_parts),
                    key: key.clone(),
                    revision,
                    dependencies,
                    diagnostics: Vec::new(),
                });
                key
            })
            .collect()
    }

    fn record_item_signature_facts(
        &mut self,
        revision: ProjectRevision,
        output: &ProjectOutput,
        project_signature_key: FrontendArtifactKey,
    ) -> Vec<FrontendArtifactKey> {
        let Some(signature_types) = output.signature_types.as_ref() else {
            return Vec::new();
        };
        let Some(hir) = output.hir.as_ref().map(|hir| &hir.hir) else {
            return Vec::new();
        };
        let mut items = hir.items.iter().map(|(item, _)| item).collect::<Vec<_>>();
        items.sort_by_key(|item| item.0);
        items
            .into_iter()
            .map(|item| {
                let key = FrontendArtifactKey::item(FrontendArtifactKind::SignatureFacts, item);
                let dependencies = self
                    .dependency_edge(project_signature_key.clone())
                    .into_iter()
                    .collect::<Vec<_>>();
                let parts =
                    if let Some(signature) = signature_types.facts.item_signatures.get(&item) {
                        item_signature_fingerprint_parts(item, signature, &signature_types.store)
                    } else {
                        hir_item_signature_fingerprint_parts(
                            item,
                            hir.items
                                .get(item)
                                .expect("HIR item key came from HIR item arena"),
                            hir,
                        )
                    };
                self.record(FrontendArtifactRecord {
                    fingerprint: self.fingerprint_for_with_parts(
                        FrontendArtifactKind::SignatureFacts,
                        &dependencies,
                        parts,
                    ),
                    key: key.clone(),
                    revision,
                    dependencies,
                    diagnostics: Vec::new(),
                });
                key
            })
            .collect()
    }

    fn record_body_effect_facts(
        &mut self,
        revision: ProjectRevision,
        mut units: Vec<UnitId>,
        body_hir: Vec<FrontendArtifactKey>,
        output: &ProjectOutput,
    ) -> Vec<FrontendArtifactKey> {
        units.sort_by_key(|unit| unit.0);
        units
            .into_iter()
            .map(|unit| {
                let body_unit = etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64);
                let key = FrontendArtifactKey::unit(FrontendArtifactKind::EffectFacts, body_unit);
                let mut dependencies = body_hir
                    .iter()
                    .filter(|body_key| {
                        matches!(
                            body_key.unit,
                            crate::artifact::FrontendUnitKey::Unit(unit_key)
                                if unit_key.id == unit.0 as u64
                        )
                    })
                    .filter_map(|dependency| self.dependency_record(dependency.clone()))
                    .collect::<Vec<_>>();
                let body_type_facts =
                    FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, body_unit);
                if let Some(dependency) = self.dependency_record(body_type_facts) {
                    dependencies.push(dependency);
                }
                let body_parts = body_fingerprint_parts(output, unit);
                self.record(FrontendArtifactRecord {
                    fingerprint: self.fingerprint_for_with_parts(
                        FrontendArtifactKind::EffectFacts,
                        &dependencies,
                        body_parts,
                    ),
                    key: key.clone(),
                    revision,
                    dependencies,
                    diagnostics: Vec::new(),
                });
                key
            })
            .collect()
    }

    fn record_body_hir_artifacts(
        &mut self,
        revision: ProjectRevision,
        output: &ProjectOutput,
    ) -> Vec<FrontendArtifactKey> {
        let Some(units) = output.units.as_ref() else {
            return Vec::new();
        };
        let mut body_units = units
            .nodes
            .iter()
            .filter_map(|(unit, node)| (node.kind == UnitKind::Body).then_some(unit))
            .collect::<Vec<_>>();
        body_units.sort_by_key(|unit| unit.0);

        body_units
            .into_iter()
            .filter_map(|unit| {
                let unit_key = etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64);
                let key = FrontendArtifactKey::unit(FrontendArtifactKind::HirBody, unit_key);
                let body_parts = body_fingerprint_parts(output, unit);
                let identity = BodyArtifactIdentity::for_unit(
                    unit,
                    output.units.as_ref().expect("unit tree should exist"),
                    output.sources.as_ref().expect("source set should exist"),
                )?;
                let parsed_source = FrontendArtifactKey::source(
                    FrontendArtifactKind::ParsedSource,
                    identity.source,
                );
                let dependencies = self
                    .dependency_edge(parsed_source)
                    .into_iter()
                    .collect::<Vec<_>>();
                self.record(FrontendArtifactRecord {
                    fingerprint: self.fingerprint_for_with_parts(
                        FrontendArtifactKind::HirBody,
                        &dependencies,
                        body_parts,
                    ),
                    key: key.clone(),
                    revision,
                    dependencies,
                    diagnostics: Vec::new(),
                });
                Some(key)
            })
            .collect()
    }

    fn fingerprint_for(
        &self,
        kind: FrontendArtifactKind,
        dependencies: &[FrontendArtifactDependency],
    ) -> ArtifactFingerprint {
        let mut parts = vec![kind.key().to_owned()];
        for dependency in dependencies {
            parts.push(dependency.key.to_cache_key().to_string());
            if let Some(fingerprint) = dependency.fingerprint {
                parts.push(fingerprint.to_string());
            }
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        fingerprint_text(&refs)
    }

    fn fingerprint_for_with_parts(
        &self,
        kind: FrontendArtifactKind,
        dependencies: &[FrontendArtifactDependency],
        extra_parts: Vec<String>,
    ) -> ArtifactFingerprint {
        let mut parts = vec![kind.key().to_owned()];
        parts.extend(extra_parts);
        for dependency in dependencies {
            parts.push(dependency.key.to_cache_key().to_string());
            if let Some(fingerprint) = dependency.fingerprint {
                parts.push(fingerprint.to_string());
            }
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        fingerprint_text(&refs)
    }

    fn dependency_record(&self, key: FrontendArtifactKey) -> Option<FrontendArtifactDependency> {
        self.get(&key).map(|record| FrontendArtifactDependency {
            key,
            fingerprint: Some(record.fingerprint),
        })
    }

    fn dependency_edge(&self, key: FrontendArtifactKey) -> Option<FrontendArtifactDependency> {
        self.get(&key).map(|_| FrontendArtifactDependency {
            key,
            fingerprint: None,
        })
    }
}

fn body_fingerprint_parts(output: &ProjectOutput, unit: UnitId) -> Vec<String> {
    let units = output.units.as_ref().expect("unit tree should exist");
    let sources = output.sources.as_ref().expect("source set should exist");
    BodyArtifactIdentity::for_unit(unit, units, sources)
        .expect("body artifact unit must resolve to a source-backed AST body")
        .fingerprint_parts(unit)
}

fn effect_pipeline_fingerprint_parts(output: &ProjectOutput) -> Vec<String> {
    let Some(artifacts) = output.effect_pipeline_artifacts.as_ref() else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    let mut tags = artifacts
        .dependency_metadata
        .tags
        .iter()
        .map(|tag| {
            format!(
                "dependency_effect_tag:{}:{}",
                tag.path.join("."),
                tag.runtime_requirement
                    .as_ref()
                    .map(|reason| format!("{reason:?}"))
                    .unwrap_or_else(|| "none".to_owned())
            )
        })
        .collect::<Vec<_>>();
    tags.sort();
    parts.extend(tags);
    let mut extensions = artifacts
        .dependency_metadata
        .extensions
        .iter()
        .map(|extension| {
            format!(
                "dependency_effect_extension:{}>{}",
                extension.child.join("."),
                extension.parent.join(".")
            )
        })
        .collect::<Vec<_>>();
    extensions.sort();
    parts.extend(extensions);
    parts
}

fn body_signature_dependencies(
    output: &ProjectOutput,
    unit: UnitId,
    item_signatures: &[FrontendArtifactKey],
) -> Vec<FrontendArtifactKey> {
    let Some(units) = output.units.as_ref() else {
        return Vec::new();
    };
    let Some(node) = units.nodes.get(unit) else {
        return Vec::new();
    };
    let UnitTarget::AstBody(body) = &node.target else {
        return Vec::new();
    };
    let Some(resolved_paths) = output.resolved_paths.as_ref() else {
        return Vec::new();
    };
    let Some(body_bindings) = output.hir_body_bindings.as_ref() else {
        return Vec::new();
    };
    let Some(item_bindings) = output.hir_item_bindings.as_ref() else {
        return Vec::new();
    };
    let Some(hir) = output.hir.as_ref().map(|hir| &hir.hir) else {
        return Vec::new();
    };

    let mut items = resolved_paths
        .expr_paths
        .iter()
        .filter(|path| body_bindings.expr_to_ast.get(&path.expr) == Some(body))
        .filter_map(|path| source_item_for_expr_path(path, output, hir, item_bindings))
        .collect::<Vec<_>>();
    items.extend(body_type_referenced_items(output, body, hir, item_bindings));
    items.sort_by_key(|item| item.0);
    items.dedup();
    items
        .into_iter()
        .map(|item| FrontendArtifactKey::item(FrontendArtifactKind::SignatureFacts, item))
        .filter(|key| item_signatures.contains(key))
        .collect()
}

fn body_type_referenced_items(
    output: &ProjectOutput,
    body: &crate::AstBodyRef,
    hir: &HirProgram,
    item_bindings: &crate::HirItemBindings,
) -> Vec<HirItemId> {
    let Some(body_bindings) = output.hir_body_bindings.as_ref() else {
        return Vec::new();
    };
    let mut collector = BodySignatureReferenceCollector {
        output,
        hir,
        item_bindings,
        items: Vec::new(),
    };
    for block in body_bindings
        .body_blocks
        .get(body)
        .into_iter()
        .flatten()
        .copied()
    {
        collector.visit_block_types(block);
    }
    for expr in body_bindings
        .body_exprs
        .get(body)
        .into_iter()
        .flatten()
        .copied()
    {
        collector.visit_expr_type_references(expr);
    }
    collector.items
}

struct BodySignatureReferenceCollector<'a> {
    output: &'a ProjectOutput,
    hir: &'a HirProgram,
    item_bindings: &'a crate::HirItemBindings,
    items: Vec<HirItemId>,
}

impl BodySignatureReferenceCollector<'_> {
    fn visit_block_types(&mut self, block: HirBlockId) {
        let Some(block) = self.hir.blocks.get(block) else {
            return;
        };
        for stmt in &block.stmts {
            self.visit_stmt_type_references(*stmt);
        }
    }

    fn visit_stmt_type_references(&mut self, stmt: etas_hir::HirStmtId) {
        let Some(stmt) = self.hir.stmts.get(stmt) else {
            return;
        };
        match stmt {
            HirStmt::Let {
                pat,
                type_annotation,
                ..
            }
            | HirStmt::Var {
                pat,
                type_annotation,
                ..
            } => {
                self.visit_pat_type_references(*pat);
                if let Some(ty) = type_annotation {
                    self.visit_type(*ty);
                }
            }
            HirStmt::For { pat, .. } => self.visit_pat_type_references(*pat),
            HirStmt::Assign { .. }
            | HirStmt::If(_)
            | HirStmt::Match(_)
            | HirStmt::While { .. }
            | HirStmt::Retry { .. }
            | HirStmt::Resume { .. }
            | HirStmt::Finish { .. }
            | HirStmt::Return { .. }
            | HirStmt::Break { .. }
            | HirStmt::Continue { .. }
            | HirStmt::Expr { .. }
            | HirStmt::Error { .. } => {}
        }
    }

    fn visit_expr_type_references(&mut self, expr: HirExprId) {
        let Some(expr) = self.hir.exprs.get(expr) else {
            return;
        };
        match expr {
            HirExpr::Record(record) => {
                if let Some(path) = &record.path {
                    self.push_path_source_item(path);
                }
            }
            HirExpr::Call { generic_args, .. } | HirExpr::MethodCall { generic_args, .. } => {
                for arg in generic_args {
                    self.visit_generic_arg(arg);
                }
            }
            HirExpr::SpecMethodCall {
                spec_path,
                spec_args,
                ..
            } => {
                self.push_path_source_item(spec_path);
                for arg in spec_args {
                    self.visit_type(*arg);
                }
            }
            HirExpr::Perform {
                action,
                generic_args,
                ..
            } => {
                self.push_path_source_item(&action.effect.path);
                for arg in generic_args {
                    self.visit_generic_arg(arg);
                }
            }
            HirExpr::Handler { handlers, .. } => {
                self.visit_handler_arms(handlers);
            }
            HirExpr::Handle { .. } => {}
            HirExpr::Lambda { params, .. } => {
                for param in params {
                    if let Some(ty) = self
                        .hir
                        .symbols
                        .get(*param)
                        .and_then(|symbol| symbol.declared_type)
                    {
                        self.visit_type(ty);
                    }
                }
            }
            HirExpr::Literal(_)
            | HirExpr::Path(_)
            | HirExpr::EmptyRecordOrMap { .. }
            | HirExpr::Tuple { .. }
            | HirExpr::Array { .. }
            | HirExpr::List { .. }
            | HirExpr::ListCons { .. }
            | HirExpr::EmptySequence { .. }
            | HirExpr::Map { .. }
            | HirExpr::Set { .. }
            | HirExpr::Range { .. }
            | HirExpr::StageCompose { .. }
            | HirExpr::Pipeline { .. }
            | HirExpr::Field { .. }
            | HirExpr::Index { .. }
            | HirExpr::Slice { .. }
            | HirExpr::Try { .. }
            | HirExpr::Unary { .. }
            | HirExpr::Binary { .. }
            | HirExpr::If { .. }
            | HirExpr::Match { .. }
            | HirExpr::Block(_)
            | HirExpr::Error { .. } => {}
        }
    }

    fn visit_pat_type_references(&mut self, pat: HirPatId) {
        let Some(pat) = self.hir.pats.get(pat) else {
            return;
        };
        match pat {
            HirPat::Tuple { elems, .. } => {
                for elem in elems {
                    self.visit_pat_type_references(*elem);
                }
            }
            HirPat::Record { path, fields, .. } => {
                if let Some(path) = path {
                    self.push_path_source_item(path);
                }
                for field in fields {
                    if let Some(pat) = field.pat {
                        self.visit_pat_type_references(pat);
                    }
                }
            }
            HirPat::Variant { path, args, .. } => {
                self.push_path_source_item(path);
                for arg in args {
                    self.visit_pat_type_references(*arg);
                }
            }
            HirPat::Binding { .. }
            | HirPat::Wildcard { .. }
            | HirPat::Literal(_)
            | HirPat::Error { .. } => {}
        }
    }

    fn visit_handler_arms(&mut self, handlers: &[etas_hir::HirHandlerArmId]) {
        for handler in handlers {
            let handler = &self.hir.handler_arms[*handler];
            self.push_path_source_item(&handler.action.effect.path);
            for arg in &handler.generic_args {
                self.visit_generic_arg(arg);
            }
            for pat in &handler.patterns {
                self.visit_pat_type_references(*pat);
            }
        }
    }

    fn visit_generic_arg(&mut self, arg: &etas_hir::HirGenericArg) {
        match arg {
            etas_hir::HirGenericArg::Type(ty) => self.visit_type(*ty),
            etas_hir::HirGenericArg::EffectRow(row) => self.visit_effect_row(row),
            etas_hir::HirGenericArg::Wildcard { .. } => {}
        }
    }

    fn visit_type(&mut self, ty: HirTypeId) {
        let Some(ty) = self.hir.types.get(ty) else {
            return;
        };
        match ty {
            HirType::Handler {
                handled,
                produced,
                result,
                ..
            } => {
                self.visit_effect_row(handled);
                if let etas_hir::HirHandlerProducedEffects::Explicit(produced) = produced {
                    self.visit_effect_row(produced);
                }
                if let Some(result) = result {
                    self.visit_type(*result);
                }
            }
            HirType::Arrow {
                effect,
                input,
                output,
                ..
            } => {
                if let Some(effect) = effect {
                    self.visit_effect_row(effect);
                }
                self.visit_type(*input);
                self.visit_type(*output);
            }
            HirType::Path { path, args, .. } => {
                self.push_path_source_item(path);
                for arg in args {
                    self.visit_type(*arg);
                }
            }
            HirType::Record { fields, .. } => {
                for field in fields {
                    self.visit_type(field.ty);
                }
            }
            HirType::Tuple { elems, .. } => {
                for elem in elems {
                    self.visit_type(*elem);
                }
            }
            HirType::Refined {
                base, predicate, ..
            } => {
                self.visit_type(*base);
                self.visit_expr_type_references(*predicate);
            }
            HirType::Primitive { .. } | HirType::Error { .. } => {}
        }
    }

    fn visit_effect_row(&mut self, row: &etas_hir::HirEffectRow) {
        for effect in &row.effects {
            self.push_path_source_item(&effect.path);
            for arg in &effect.args {
                if let HirEffectArg::Type(ty) = arg {
                    self.visit_type(*ty);
                }
            }
        }
    }

    fn push_path_source_item(&mut self, path: &etas_hir::ResolvedPath) {
        if let Some(item) = source_item_for_path(path, self.output, self.hir, self.item_bindings) {
            self.items.push(item);
        }
    }
}

fn source_item_for_expr_path(
    path: &crate::HirExprPathResolution,
    output: &ProjectOutput,
    hir: &HirProgram,
    item_bindings: &crate::HirItemBindings,
) -> Option<HirItemId> {
    match &path.result {
        HirPathResolution::WildcardImport { target } => {
            let name = single_segment_path_name(&path.path)?;
            source_item_for_module_target_export(target, name, output, item_bindings)
        }
        HirPathResolution::PartialImport {
            target: ImportTarget::Module(module),
            remaining,
            ..
        } if remaining.len() == 1 => {
            source_item_for_module_target_export(module, &remaining[0], output, item_bindings)
        }
        _ => source_item_for_resolution(&path.result, hir, item_bindings),
    }
}

fn source_item_for_resolution(
    resolution: &HirPathResolution,
    hir: &HirProgram,
    item_bindings: &crate::HirItemBindings,
) -> Option<HirItemId> {
    match resolution {
        HirPathResolution::DirectSymbol(symbol) => symbol_source_item(*symbol, hir),
        HirPathResolution::PartialSymbol { prefix, .. } => symbol_source_item(*prefix, hir),
        HirPathResolution::ExplicitImport { target }
        | HirPathResolution::PartialImport { target, .. } => {
            import_target_source_item(target, item_bindings)
        }
        HirPathResolution::WildcardImport { .. }
        | HirPathResolution::AmbiguousWildcard { .. }
        | HirPathResolution::PartiallyResolved { .. }
        | HirPathResolution::Unresolved => None,
    }
}

fn source_item_for_path(
    path: &etas_hir::ResolvedPath,
    output: &ProjectOutput,
    hir: &HirProgram,
    item_bindings: &crate::HirItemBindings,
) -> Option<HirItemId> {
    match &path.resolution {
        ResolveResult::Resolved(symbol) => {
            if hir.symbols.get(*symbol).is_some_and(source_import_alias)
                && path.segments.len() == 1
                && let Some(name) = path.segments.first().map(|segment| segment.name.as_str())
                && let Some(target) =
                    explicit_import_target_for_name(output, path.span.source, name)
            {
                return import_target_source_item(target, item_bindings);
            }
            symbol_source_item(*symbol, hir)
        }
        ResolveResult::PartiallyResolved(partial) => {
            let prefix = partial.resolved_prefix?;
            if partial.resolved_segments == 1
                && hir.symbols.get(prefix).is_some_and(source_import_alias)
                && let Some(name) = path.segments.first().map(|segment| segment.name.as_str())
                && let Some(target) =
                    explicit_import_target_for_name(output, path.span.source, name)
            {
                return import_target_source_item(target, item_bindings);
            }
            symbol_source_item(prefix, hir)
        }
        ResolveResult::Unresolved => {
            let name = single_segment_path_name(path)?;
            source_item_for_unambiguous_wildcard_name(output, path.span.source, name, item_bindings)
        }
        ResolveResult::Ambiguous(_) => None,
    }
}

fn single_segment_path_name(path: &etas_hir::ResolvedPath) -> Option<&str> {
    (path.segments.len() == 1).then(|| path.segments[0].name.as_str())
}

fn source_item_for_unambiguous_wildcard_name(
    output: &ProjectOutput,
    source: SourceId,
    name: &str,
    item_bindings: &crate::HirItemBindings,
) -> Option<HirItemId> {
    let module = output.modules.as_ref()?.by_source.get(&source).copied()?;
    let part = output
        .modules
        .as_ref()?
        .parts
        .iter()
        .find_map(|(_, part)| (part.source == source).then_some(part.id))?;
    let mut items = output
        .resolved_imports
        .as_ref()?
        .wildcard_imports
        .iter()
        .filter(|import| {
            import.from == module
                && import.from_part == part
                && import.exported_names.iter().any(|export| export == name)
        })
        .filter_map(|import| {
            source_item_for_module_target_export(&import.target_module, name, output, item_bindings)
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.0);
    items.dedup();
    match items.as_slice() {
        [item] => Some(*item),
        _ => None,
    }
}

fn source_item_for_module_target_export(
    target: &ResolvedModuleTarget,
    name: &str,
    output: &ProjectOutput,
    item_bindings: &crate::HirItemBindings,
) -> Option<HirItemId> {
    let ResolvedModuleTarget::Source { module, .. } = target else {
        return None;
    };
    let item = output
        .modules
        .as_ref()?
        .modules
        .get(*module)?
        .visibility_exports
        .items
        .get(name)?;
    item_bindings.ast_to_hir.get(&item.item).copied()
}

fn explicit_import_target_for_name<'a>(
    output: &'a ProjectOutput,
    source: SourceId,
    name: &str,
) -> Option<&'a ImportTarget> {
    let module = output.modules.as_ref()?.by_source.get(&source).copied()?;
    let part = output
        .modules
        .as_ref()?
        .parts
        .iter()
        .find_map(|(_, part)| (part.source == source).then_some(part.id))?;
    output
        .resolved_imports
        .as_ref()?
        .imports
        .iter()
        .find(|import| {
            import.from == module && import.from_part == part && import.local_name == name
        })
        .map(|import| &import.target)
}

fn symbol_source_item(symbol: etas_hir::SymbolId, hir: &HirProgram) -> Option<HirItemId> {
    match &hir.symbols.get(symbol)?.def {
        SymbolDef::Item { item } | SymbolDef::TopLevelLet { item, .. } => Some(*item),
        _ => None,
    }
}

fn source_import_alias(symbol: &etas_hir::Symbol) -> bool {
    matches!(
        &symbol.def,
        SymbolDef::ImportAlias {
            origin: etas_hir::ImportAliasOrigin::SourceImport,
            ..
        }
    )
}

fn import_target_source_item(
    target: &ImportTarget,
    item_bindings: &crate::HirItemBindings,
) -> Option<HirItemId> {
    let ImportTarget::SourceItem { item, .. } = target else {
        return None;
    };
    item_bindings.ast_to_hir.get(item).copied()
}

fn item_signature_fingerprint_parts(
    item: HirItemId,
    signature: &ItemSignature,
    store: &TypeStore,
) -> Vec<String> {
    let mut parts = vec![format!("item:{}", item.0)];
    match signature {
        ItemSignature::Flow(signature) => {
            parts.push("kind:flow".to_owned());
            parts.extend(type_list_parts("param", &signature.params, store));
            parts.push(format!(
                "output:{}",
                type_fingerprint_part(signature.output, store)
            ));
            parts.push(effect_row_part(&signature.effects, store));
        }
        ItemSignature::Agent(signature) => {
            parts.push("kind:agent".to_owned());
            parts.extend(type_list_parts("input", &signature.params, store));
            parts.push(format!(
                "output:{}",
                type_fingerprint_part(signature.output, store)
            ));
            parts.push(effect_row_part(&signature.effects, store));
        }
        ItemSignature::Tool(signature) => {
            parts.push("kind:tool".to_owned());
            parts.extend(type_list_parts("input", &signature.params, store));
            parts.push(format!(
                "output:{}",
                type_fingerprint_part(signature.output, store)
            ));
            parts.push(effect_row_part(&signature.effects, store));
        }
        ItemSignature::TopLevelLet(signature) => {
            parts.push("kind:top_level_let".to_owned());
            parts.push(format!(
                "type:{}",
                type_fingerprint_part(signature.ty, store)
            ));
        }
    }
    parts
}

fn hir_item_signature_fingerprint_parts(
    item: HirItemId,
    hir_item: &HirItem,
    hir: &HirProgram,
) -> Vec<String> {
    let mut parts = vec![format!("item:{}", item.0)];
    match hir_item {
        HirItem::TypeAlias(item) => {
            parts.push("kind:type_alias".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            parts.push(format!(
                "target:{}",
                hir_type_signature_part(item.target, hir)
            ));
        }
        HirItem::Type(item) => {
            parts.push("kind:type".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            match item.body {
                etas_hir::HirTypeDeclBody::Bodyless => {
                    parts.push("body:bodyless".to_owned());
                }
                etas_hir::HirTypeDeclBody::Representation(ty) => {
                    parts.push(format!(
                        "representation:{}",
                        hir_type_signature_part(ty, hir)
                    ));
                }
            }
        }
        HirItem::Enum(item) => {
            parts.push("kind:enum".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            for (index, variant) in item.variants.iter().enumerate() {
                parts.push(format!(
                    "variant:{index}:{}({})",
                    symbol_name(hir, variant.symbol),
                    variant
                        .fields
                        .iter()
                        .map(|field| hir_type_signature_part(*field, hir))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
        }
        HirItem::Effect(item) => {
            parts.push("kind:effect".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            if let Some(extends) = &item.extends {
                parts.push(format!("extends:{}", path_signature_part(&extends.path)));
            }
            for (index, action) in item.body.actions().iter().enumerate() {
                parts.push(format!(
                    "action:{index}:{}({})->{}",
                    symbol_name(hir, action.symbol),
                    action
                        .params
                        .iter()
                        .map(|param| symbol_declared_type_part(hir, *param))
                        .collect::<Vec<_>>()
                        .join(","),
                    hir_type_signature_part(action.return_type, hir)
                ));
            }
        }
        HirItem::Protocol(item) => {
            parts.push("kind:protocol".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            for (index, message) in item.messages.iter().enumerate() {
                parts.push(format!(
                    "message:{index}:{}->{}:{}",
                    path_signature_part(&message.from),
                    path_signature_part(&message.to),
                    hir_type_signature_part(message.payload, hir)
                ));
            }
        }
        HirItem::Spec(item) => {
            parts.push("kind:spec".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            for (index, bound) in item.bounds.iter().enumerate() {
                parts.push(format!(
                    "super_spec:{index}:{}",
                    path_signature_part(&bound.path)
                ));
                parts.extend(bound.args.iter().enumerate().map(|(arg_index, ty)| {
                    format!(
                        "super_spec_arg:{index}:{arg_index}:{}",
                        hir_type_signature_part(*ty, hir)
                    )
                }));
            }
            if let Some(callable) = &item.callable {
                parts.push(format!(
                    "callable:{}=>{}:{}",
                    hir_type_signature_part(callable.input, hir),
                    hir_type_signature_part(callable.output, hir),
                    callable
                        .effects
                        .as_ref()
                        .map(|effects| hir_effect_row_signature_part(effects, hir))
                        .unwrap_or_else(|| "effects:none".to_owned())
                ));
            }
            for (index, item) in item.items.iter().enumerate() {
                match item {
                    etas_hir::HirSpecItem::FlowSignature(signature) => {
                        parts.push(format!(
                            "flow_signature:{index}:{}({})->{}",
                            symbol_name(hir, signature.symbol),
                            signature
                                .params
                                .iter()
                                .map(|param| symbol_declared_type_part(hir, *param))
                                .collect::<Vec<_>>()
                                .join(","),
                            signature.return_type.map_or_else(
                                || "<infer>".to_owned(),
                                |ty| { hir_type_signature_part(ty, hir) }
                            )
                        ));
                    }
                    etas_hir::HirSpecItem::Error { span } => {
                        parts.push(format!("error_spec_item:{index}:{:?}", span.range.start));
                    }
                }
            }
        }
        HirItem::Impl(item) => {
            parts.push("kind:impl".to_owned());
            match &item.target {
                etas_hir::HirImplTarget::Inherent { target, type_args } => {
                    parts.push(format!("target:{}", path_signature_part(target)));
                    parts.extend(type_args.iter().enumerate().map(|(index, ty)| {
                        format!("type_arg:{index}:{}", hir_type_signature_part(*ty, hir))
                    }));
                }
                etas_hir::HirImplTarget::SpecSatisfaction { specs, self_type } => {
                    for (spec_index, spec_ref) in specs.iter().enumerate() {
                        parts.push(format!(
                            "spec:{spec_index}:{}",
                            path_signature_part(&spec_ref.spec_path)
                        ));
                        parts.extend(spec_ref.spec_args.iter().enumerate().map(
                            |(arg_index, ty)| {
                                format!(
                                    "spec_arg:{spec_index}:{arg_index}:{}",
                                    hir_type_signature_part(*ty, hir)
                                )
                            },
                        ));
                    }
                    parts.push(format!("self:{}", hir_type_signature_part(*self_type, hir)));
                }
                etas_hir::HirImplTarget::Error => {
                    parts.push("target:<error>".to_owned());
                }
            }
        }
        HirItem::Flow(item) => {
            parts.push("kind:flow".to_owned());
            parts.push(format!("symbol:{}", symbol_name(hir, item.symbol)));
            push_declaration_conformance_signature_parts(
                &mut parts,
                "flow_conformance",
                &item.conformances,
                hir,
            );
        }
        HirItem::Agent(item) => {
            parts.push("kind:agent".to_owned());
            push_declaration_conformance_signature_parts(
                &mut parts,
                "agent_conformance",
                &item.conformances,
                hir,
            );
        }
        HirItem::Tool(item) => {
            parts.push("kind:tool".to_owned());
            push_declaration_conformance_signature_parts(
                &mut parts,
                "tool_conformance",
                &item.conformances,
                hir,
            );
        }
        HirItem::TopLevelLet(_) | HirItem::Error { .. } => {
            parts.push(format!("kind:{:?}", std::mem::discriminant(hir_item)));
        }
    }
    parts
}

fn push_declaration_conformance_signature_parts(
    parts: &mut Vec<String>,
    label: &str,
    conformances: &[etas_hir::HirDeclarationConformance],
    hir: &HirProgram,
) {
    for (index, conformance) in conformances.iter().enumerate() {
        match &conformance.target {
            etas_hir::HirDeclarationConformanceTarget::Path(spec_ref) => {
                parts.push(format!(
                    "{label}:{index}:path:{}",
                    path_signature_part(&spec_ref.spec_path)
                ));
                parts.extend(
                    spec_ref
                        .spec_args
                        .iter()
                        .enumerate()
                        .map(|(arg_index, ty)| {
                            format!(
                                "{label}_arg:{index}:{arg_index}:{}",
                                hir_type_signature_part(*ty, hir)
                            )
                        }),
                );
            }
            etas_hir::HirDeclarationConformanceTarget::InlineTraceSpec(expr) => {
                parts.push(format!(
                    "{label}:{index}:inline:{}",
                    hir_spec_expr_signature_part(expr, hir)
                ));
            }
            etas_hir::HirDeclarationConformanceTarget::Error { span } => {
                parts.push(format!("{label}:{index}:error:{:?}", span.range.start));
            }
        }
    }
}

fn hir_spec_expr_signature_part(expr: &etas_hir::HirSpecExpr, hir: &HirProgram) -> String {
    match expr {
        etas_hir::HirSpecExpr::Atom(pattern) => {
            format!("atom({})", hir_effect_ref_signature_part(pattern, hir))
        }
        etas_hir::HirSpecExpr::Allow { pattern, .. } => {
            format!("allow({})", hir_effect_ref_signature_part(pattern, hir))
        }
        etas_hir::HirSpecExpr::Deny { pattern, .. } => {
            format!("deny({})", hir_effect_ref_signature_part(pattern, hir))
        }
        etas_hir::HirSpecExpr::And { lhs, rhs, .. } => format!(
            "and({},{})",
            hir_spec_expr_signature_part(lhs, hir),
            hir_spec_expr_signature_part(rhs, hir)
        ),
        etas_hir::HirSpecExpr::Or { lhs, rhs, .. } => format!(
            "or({},{})",
            hir_spec_expr_signature_part(lhs, hir),
            hir_spec_expr_signature_part(rhs, hir)
        ),
        etas_hir::HirSpecExpr::Before { before, after, .. } => format!(
            "before({},{})",
            hir_spec_expr_signature_part(before, hir),
            hir_spec_expr_signature_part(after, hir)
        ),
        etas_hir::HirSpecExpr::After { after, before, .. } => format!(
            "after({},{})",
            hir_spec_expr_signature_part(after, hir),
            hir_spec_expr_signature_part(before, hir)
        ),
    }
}

fn hir_type_signature_part(ty: HirTypeId, hir: &HirProgram) -> String {
    match hir.types.get(ty).expect("HIR type id should resolve") {
        HirType::Handler {
            handled,
            produced,
            result,
            ..
        } => {
            let produced = match produced {
                etas_hir::HirHandlerProducedEffects::Infer => "produced:infer".to_owned(),
                etas_hir::HirHandlerProducedEffects::Explicit(row) => {
                    format!("produced:{}", hir_effect_row_signature_part(row, hir))
                }
            };
            let result = result
                .map(|result| hir_type_signature_part(result, hir))
                .unwrap_or_else(|| "result:none".to_owned());
            format!(
                "handler({})->{}:{}",
                hir_effect_row_signature_part(handled, hir),
                produced,
                result
            )
        }
        HirType::Arrow {
            effect,
            input,
            output,
            ..
        } => format!(
            "arrow({})->{}:{}",
            hir_type_signature_part(*input, hir),
            hir_type_signature_part(*output, hir),
            effect
                .as_ref()
                .map(|effect| hir_effect_row_signature_part(effect, hir))
                .unwrap_or_else(|| "effects:none".to_owned())
        ),
        HirType::Primitive { kind, .. } => format!("primitive:{kind:?}"),
        HirType::Path { path, args, .. } => format!(
            "path:{}<{}>",
            path_signature_part(path),
            args.iter()
                .map(|arg| hir_type_signature_part(*arg, hir))
                .collect::<Vec<_>>()
                .join(",")
        ),
        HirType::Record { fields, .. } => {
            let mut fields = fields
                .iter()
                .map(|field| {
                    format!(
                        "{}:{}",
                        symbol_name(hir, field.symbol),
                        hir_type_signature_part(field.ty, hir)
                    )
                })
                .collect::<Vec<_>>();
            fields.sort();
            format!("record{{{}}}", fields.join(","))
        }
        HirType::Tuple { elems, .. } => format!(
            "tuple({})",
            elems
                .iter()
                .map(|elem| hir_type_signature_part(*elem, hir))
                .collect::<Vec<_>>()
                .join(",")
        ),
        HirType::Refined { base, .. } => {
            format!("refined<{}>", hir_type_signature_part(*base, hir))
        }
        HirType::Error { .. } => "error".to_owned(),
    }
}

fn hir_effect_row_signature_part(row: &etas_hir::HirEffectRow, hir: &HirProgram) -> String {
    let mut effects = row
        .effects
        .iter()
        .map(|effect| hir_effect_ref_signature_part(effect, hir))
        .collect::<Vec<_>>();
    effects.sort();
    format!("effects:{}", effects.join("|"))
}

fn hir_effect_ref_signature_part(effect: &etas_hir::HirEffectRef, hir: &HirProgram) -> String {
    format!(
        "{}<{}>",
        path_signature_part(&effect.path),
        effect
            .args
            .iter()
            .map(|arg| hir_effect_arg_signature_part(arg, hir))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn hir_effect_arg_signature_part(arg: &HirEffectArg, hir: &HirProgram) -> String {
    match arg {
        HirEffectArg::Type(ty) => format!("type:{}", hir_type_signature_part(*ty, hir)),
        HirEffectArg::Path(path) => format!("path:{}", path_signature_part(path)),
        HirEffectArg::Wildcard { .. } => "wildcard:_".to_owned(),
        HirEffectArg::String { value, .. } => format!("string:{value:?}"),
        HirEffectArg::Int { text, .. } => format!("int:{text}"),
    }
}

fn symbol_declared_type_part(hir: &HirProgram, symbol: etas_hir::SymbolId) -> String {
    hir.symbols
        .get(symbol)
        .and_then(|symbol| symbol.declared_type)
        .map(|ty| hir_type_signature_part(ty, hir))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn symbol_name(hir: &HirProgram, symbol: etas_hir::SymbolId) -> String {
    hir.symbols
        .get(symbol)
        .map(|symbol| symbol.name.clone())
        .unwrap_or_else(|| format!("symbol:{}", symbol.0))
}

fn path_signature_part(path: &etas_hir::ResolvedPath) -> String {
    path.segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn type_list_parts(prefix: &str, types: &[TypeId], store: &TypeStore) -> Vec<String> {
    types
        .iter()
        .enumerate()
        .map(|(index, ty)| format!("{prefix}:{index}:{}", type_fingerprint_part(*ty, store)))
        .collect()
}

fn effect_row_part(row: &Option<etas_types::EffectRowRef>, store: &TypeStore) -> String {
    let Some(row) = row else {
        return "effects:none".to_owned();
    };
    let mut effects = row
        .effects
        .iter()
        .map(|effect| {
            let args = effect
                .args
                .iter()
                .map(|arg| effect_arg_fingerprint_part(arg, store))
                .collect::<Vec<_>>()
                .join(",");
            format!("{}({})", effect.name, args)
        })
        .collect::<Vec<_>>();
    effects.sort();
    format!("effects:{}", effects.join("|"))
}

fn required_effect_row_part(row: &etas_types::EffectRowRef, store: &TypeStore) -> String {
    effect_row_part(&Some(row.clone()), store)
}

fn effect_arg_fingerprint_part(arg: &EffectArgRef, store: &TypeStore) -> String {
    match arg {
        EffectArgRef::Type(ty) => format!("type:{}", type_fingerprint_part(*ty, store)),
        EffectArgRef::Wildcard => "wildcard:_".to_owned(),
        EffectArgRef::String(value) => format!("string:{value:?}"),
        EffectArgRef::Int(text) => format!("int:{text}"),
        EffectArgRef::Path(segments) => format!("path:{}", segments.join(".")),
    }
}

fn type_fingerprint_part(ty: TypeId, store: &TypeStore) -> String {
    match store.get(ty).expect("signature type id should resolve") {
        Type::Primitive(primitive) => format!("primitive:{primitive:?}"),
        Type::IntegerLiteral { text } => format!("integer_literal:{text}"),
        Type::Var(var) => format!("var:{}", var.0),
        Type::Array(inner) => format!("array<{}>", type_fingerprint_part(*inner, store)),
        Type::List(inner) => format!("list<{}>", type_fingerprint_part(*inner, store)),
        Type::Map { key, value } => format!(
            "map<{},{}>",
            type_fingerprint_part(*key, store),
            type_fingerprint_part(*value, store)
        ),
        Type::Set(inner) => format!("set<{}>", type_fingerprint_part(*inner, store)),
        Type::Range { index } => format!("range<{}>", type_fingerprint_part(*index, store)),
        Type::Slice(inner) => format!("slice<{}>", type_fingerprint_part(*inner, store)),
        Type::Option(inner) => format!("option<{}>", type_fingerprint_part(*inner, store)),
        Type::Result { ok, err } => format!(
            "result<{},{}>",
            type_fingerprint_part(*ok, store),
            type_fingerprint_part(*err, store)
        ),
        Type::Record(record) => {
            let mut fields = record
                .fields
                .iter()
                .map(|field| format!("{}:{}", field.name, type_fingerprint_part(field.ty, store)))
                .collect::<Vec<_>>();
            fields.sort();
            format!("record{{{}}}", fields.join(","))
        }
        Type::Tuple(items) => format!(
            "tuple({})",
            items
                .iter()
                .map(|item| type_fingerprint_part(*item, store))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Enum(reference) => format!("enum:{}", reference.name),
        Type::Function(flow) => format!(
            "function({})->{}:{}",
            flow.input
                .iter()
                .map(|input| type_fingerprint_part(*input, store))
                .collect::<Vec<_>>()
                .join(","),
            type_fingerprint_part(flow.output, store),
            effect_row_part(&flow.effects, store)
        ),
        Type::Handler(handler) => {
            let produced = match &handler.produced {
                etas_types::HandlerProducedEffects::Infer => "produced:infer".to_owned(),
                etas_types::HandlerProducedEffects::Explicit(row) => {
                    format!("produced:{}", required_effect_row_part(row, store))
                }
            };
            let result = handler
                .result
                .map(|result| type_fingerprint_part(result, store))
                .unwrap_or_else(|| "result:none".to_owned());
            format!(
                "handler({})->{}:{}",
                required_effect_row_part(&handler.handled, store),
                produced,
                result
            )
        }
        Type::Named(reference) => format!("named:{}", reference.name),
        Type::Nominal(reference) => format!(
            "nominal:{}:{}",
            reference.name,
            reference
                .representation
                .map(|representation| type_fingerprint_part(representation, store))
                .unwrap_or_else(|| "bodyless".to_owned())
        ),
        Type::Applied { constructor, args } => format!(
            "applied:{}<{}>",
            constructor.0,
            args.iter()
                .map(|arg| type_fingerprint_part(*arg, store))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Refined { base, predicate } => {
            format!(
                "refined<{},{}>",
                type_fingerprint_part(*base, store),
                predicate.0
            )
        }
        Type::Trust { wrapper, inner } => {
            format!(
                "trust:{wrapper:?}<{}>",
                type_fingerprint_part(*inner, store)
            )
        }
        Type::Schema(inner) => format!("schema<{}>", type_fingerprint_part(*inner, store)),
        Type::Prompt => "prompt".to_owned(),
        Type::PromptPart => "prompt_part".to_owned(),
        Type::Message(inner) => format!("message<{}>", type_fingerprint_part(*inner, store)),
        Type::MemorySelection(inner) => {
            format!("memory_selection<{}>", type_fingerprint_part(*inner, store))
        }
        Type::Store { key, value } => format!(
            "store<{},{}>",
            type_fingerprint_part(*key, store),
            type_fingerprint_part(*value, store)
        ),
        Type::MemoryPlace(place) => format!("memory_place:{}", place.segments.join(".")),
        Type::MemoryRegion(schema) => {
            format!("memory_region<{}>", type_fingerprint_part(*schema, store))
        }
        Type::ResourceHandle(handle) => resource_handle_part(handle, store),
    }
}

fn resource_handle_part(handle: &etas_types::ResourceHandleType, store: &TypeStore) -> String {
    match handle {
        etas_types::ResourceHandleType::MemoryRegion { schema } => {
            format!(
                "resource:memory_region<{}>",
                type_fingerprint_part(*schema, store)
            )
        }
        etas_types::ResourceHandleType::ExternalTool { signature } => {
            format!(
                "resource:external_tool<{}>",
                type_fingerprint_part(*signature, store)
            )
        }
        etas_types::ResourceHandleType::Other { name, args } => format!(
            "resource:other:{}<{}>",
            name,
            args.iter()
                .map(|arg| type_fingerprint_part(*arg, store))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn source_fingerprint(source: &CoreSourceFile) -> ArtifactFingerprint {
    let path = source
        .path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    fingerprint_text(&[&source.id.0.to_string(), &path, source.text.as_ref()])
}

fn source_set_fingerprint(sources: &SourceSet) -> ArtifactFingerprint {
    let mut parts = vec![
        sources.project_root.to_string_lossy().to_string(),
        sources.source_root.to_string_lossy().to_string(),
        sources.environment_fingerprint.clone(),
        sources.external_modules_fingerprint.clone(),
    ];
    for source in &sources.files {
        parts.push(source.id.0.to_string());
        parts.push(
            source
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
        );
        parts.push(source.text.to_string());
        parts.push(format!("{:?}", source.kind));
    }
    let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
    fingerprint_text(&refs)
}

pub(in crate::session) fn diagnostic_sources(output: &ProjectOutput) -> Vec<SourceId> {
    let mut sources = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.primary.span.source)
        .collect::<Vec<_>>();
    sources.extend(output.parsed_sources.iter().map(|parsed| parsed.source));
    sources.sort_by_key(|source| source.0);
    sources.dedup();
    sources
}
