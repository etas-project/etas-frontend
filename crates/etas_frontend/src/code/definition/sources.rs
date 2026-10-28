use std::{collections::HashMap, sync::Arc};

use etas_core::{SourceFile, SourceId, Span};
use etas_hir::{HirItem, HirItemId, HirProgram, SymbolId, Visibility};
use etas_syntax::{Parse, ast};

use super::ReflectionOptions;
use crate::{
    AstBodyRef, HirBodyBindings, HirItemBindings, ModuleCatalog, ModuleExportTarget, ParsedSource,
    SourceKind, SourceSet,
};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct DefinitionSourceUnit {
    pub file: Arc<SourceFile>,
    pub program: Arc<Parse<ast::Program>>,
    pub kind: SourceKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct BodyExposure {
    pub retained: bool,
    pub exported: bool,
}

/// Native-only lexical source archive. Its ASTs are shared with the parser
/// artifacts; public body access is separately governed by `exposure`.
#[derive(Debug, etas_core::RetainedSize)]
pub struct DefinitionSources {
    source_set: Arc<SourceSet>,
    parsed_sources: Arc<[ParsedSource]>,
    files: HashMap<SourceId, DefinitionSourceUnit>,
    bodies: HashMap<HirItemId, AstBodyRef>,
    exposure: HashMap<HirItemId, BodyExposure>,
    revisions: HashMap<Span, Arc<crate::code::draft::NativeBodyRevision>>,
    overlaid_bodies: HashMap<HirItemId, Arc<crate::code::draft::NativeBodyRevision>>,
}

#[derive(Debug)]
pub(crate) struct ReflectionConfigurationError {
    pub entry: String,
    pub reason: String,
}

pub(crate) struct DefinitionSourceCaptureInput<'a> {
    pub parsed: &'a [ParsedSource],
    pub sources: &'a Arc<SourceSet>,
    pub hir: &'a HirProgram,
    pub bodies: &'a HirBodyBindings,
    pub items: &'a HirItemBindings,
    pub catalog: &'a ModuleCatalog,
    pub facts: &'a etas_types::TypeFacts,
    pub options: &'a ReflectionOptions,
    pub revisions: &'a HashMap<Span, Arc<crate::code::draft::NativeBodyRevision>>,
    pub original: Option<&'a DefinitionSources>,
}

impl DefinitionSources {
    pub(crate) fn capture(
        input: DefinitionSourceCaptureInput<'_>,
    ) -> Result<Self, ReflectionConfigurationError> {
        let DefinitionSourceCaptureInput {
            parsed,
            sources: source_set,
            hir,
            bodies: bindings,
            items: item_bindings,
            catalog,
            facts,
            options,
            revisions,
            original,
        } = input;
        let error = |entry: String, reason: &str| ReflectionConfigurationError {
            entry,
            reason: reason.into(),
        };
        let mut files = HashMap::new();
        let source_files = source_set
            .files
            .iter()
            .map(|source| (source.id, source))
            .collect::<HashMap<_, _>>();
        for source in parsed {
            let file = source_files.get(&source.source).ok_or_else(|| {
                error(
                    "<source archive>".into(),
                    "parsed source has no source-set identity",
                )
            })?;
            if source.source_file.id != source.source
                || source.parse.value.span.source != source.source
                || source.source_file.text() != file.text.as_ref()
            {
                return Err(error(
                    "<source archive>".into(),
                    "parsed AST and immutable source content disagree",
                ));
            }
            if files
                .insert(
                    source.source,
                    DefinitionSourceUnit {
                        file: source.source_file.clone(),
                        program: source.parse.clone(),
                        kind: file.kind.clone(),
                    },
                )
                .is_some()
            {
                return Err(error(
                    "<source archive>".into(),
                    "duplicate source identity",
                ));
            }
        }
        let mut bodies = HashMap::new();
        for (body, item) in &bindings.body_items {
            if !matches!(
                hir.items.get(*item),
                Some(HirItem::Flow(_) | HirItem::Agent(_) | HirItem::Tool(_))
            ) {
                continue;
            }
            if !bindings.ast_to_root_block.contains_key(body) {
                continue;
            }
            if !files.contains_key(&body.source) {
                return Err(error(
                    "<source archive>".into(),
                    "callable body has no immutable parsed source",
                ));
            }
            if bodies.insert(*item, body.clone()).is_some() {
                return Err(error(
                    "<source archive>".into(),
                    "callable has multiple native body slots",
                ));
            }
        }
        let mut names = HashMap::<Vec<String>, Vec<(HirItemId, SymbolId)>>::new();
        for (item, decl) in hir.items.iter() {
            if !matches!(
                decl,
                HirItem::Flow(_) | HirItem::Agent(_) | HirItem::Tool(_)
            ) {
                continue;
            }
            let symbol = decl.symbol().ok_or_else(|| {
                error(
                    "<source archive>".into(),
                    "callable has no declaration symbol",
                )
            })?;
            let path = hir.declaration_path(symbol).ok_or_else(|| {
                error(
                    "<source archive>".into(),
                    "callable has no canonical declaration path",
                )
            })?;
            names.entry(path).or_default().push((item, symbol));
        }
        for method in facts.agent_methods.values() {
            let path = hir.declaration_path(method.symbol).ok_or_else(|| {
                error(
                    "<source archive>".into(),
                    "agent method has no canonical declaration path",
                )
            })?;
            let targets = names.entry(path).or_default();
            let target = (method.body, method.symbol);
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        for (_, module) in catalog.modules.iter() {
            for export in module.exports.items.values() {
                let ModuleExportTarget::Source { item, .. } = &export.target else {
                    continue;
                };
                let item = item_bindings.ast_to_hir.get(item).copied().ok_or_else(|| {
                    error(
                        "<source archive>".into(),
                        "resolved source export has no HIR declaration identity",
                    )
                })?;
                let Some(decl @ (HirItem::Agent(_) | HirItem::Flow(_) | HirItem::Tool(_))) =
                    hir.items.get(item)
                else {
                    continue;
                };
                let symbol = decl.symbol().ok_or_else(|| {
                    error(
                        "<source archive>".into(),
                        "resolved callable export has no symbol",
                    )
                })?;
                let mut path = module.path.segments.clone();
                path.push(export.name.clone());
                let targets = names.entry(path).or_default();
                if !targets.contains(&(item, symbol)) {
                    targets.push((item, symbol));
                }
            }
        }
        let mut exposure = HashMap::<HirItemId, BodyExposure>::new();
        for (exported, entries) in [
            (false, &options.retain_bodies),
            (true, &options.export_bodies),
        ] {
            for entry in entries {
                let path = entry.split('.').map(str::to_owned).collect::<Vec<_>>();
                let targets = names.get(&path).ok_or_else(|| error(entry.clone(), "entry must resolve to an owned canonical agent, flow, tool, or agent method"))?;
                let [(item, symbol)] = targets.as_slice() else {
                    return Err(error(
                        entry.clone(),
                        "entry names multiple implementations; select an unambiguous callable",
                    ));
                };
                let data = hir
                    .symbols
                    .get(*symbol)
                    .ok_or_else(|| error(entry.clone(), "declaration symbol is missing"))?;
                let source = files
                    .get(&data.definition_span.source)
                    .ok_or_else(|| error(entry.clone(), "declaration has no owned source"))?;
                if matches!(
                    source.kind,
                    SourceKind::DependencySourceOverlay { .. } | SourceKind::VirtualStd
                ) {
                    return Err(error(
                        entry.clone(),
                        "a package cannot retain or export another package's body",
                    ));
                }
                if exported && data.visibility != Visibility::Public {
                    return Err(error(
                        entry.clone(),
                        "only a public item can export its source body",
                    ));
                }
                let body = if matches!(hir.items.get(*item), Some(HirItem::Agent(_))) {
                    match facts.item_signatures.get(item) {
                        Some(etas_types::ItemSignature::Agent(agent)) => agent
                            .default_entry
                            .as_ref()
                            .map(|default| {
                                let method = default
                                    .method
                                    .and_then(|method| facts.agent_methods.get(&method))
                                    .ok_or_else(|| {
                                        error(
                                            entry.clone(),
                                            "default entry has no canonical method body",
                                        )
                                    })?;
                                if method.agent != *item {
                                    return Err(error(
                                        entry.clone(),
                                        "default method belongs to another agent",
                                    ));
                                }
                                Ok(method.body)
                            })
                            .transpose()?,
                        _ => {
                            return Err(error(
                                entry.clone(),
                                "agent has no checked component signature",
                            ));
                        }
                    }
                } else {
                    Some(*item)
                };
                if let Some(body) = body {
                    if let Some(body) = bodies.get(&body)
                        && files.get(&body.source).is_some_and(|source| {
                            matches!(
                                source.kind,
                                SourceKind::DependencySourceOverlay { .. } | SourceKind::VirtualStd
                            )
                        })
                    {
                        return Err(error(
                            entry.clone(),
                            "the selected body is owned by another package",
                        ));
                    }
                    let flags = exposure.entry(body).or_default();
                    flags.retained = true;
                    flags.exported |= exported;
                }
            }
        }
        let mut overlaid_bodies = HashMap::new();
        for (item, slot) in &bodies {
            let declaration = hir
                .items
                .get(*item)
                .ok_or_else(|| error("<body overlay>".into(), "overlaid callable is missing"))?
                .span();
            if let Some(revision) = revisions.get(&declaration) {
                if revision.slot.source != slot.source
                    || revision.slot.item.index != slot.item.index
                    || revision.slot.impl_member != slot.impl_member
                {
                    return Err(error(
                        "<body overlay>".into(),
                        "body revision changed its authoritative AST slot",
                    ));
                }
                overlaid_bodies.insert(*item, revision.clone());
            }
        }
        if overlaid_bodies.len() != revisions.len() {
            return Err(error(
                "<body overlay>".into(),
                "not every body revision has a checked callable owner",
            ));
        }
        Ok(Self {
            source_set: source_set.clone(),
            parsed_sources: original
                .filter(|archive| {
                    archive.parsed_sources.len() == parsed.len()
                        && archive
                            .parsed_sources
                            .iter()
                            .zip(parsed)
                            .all(|(before, after)| {
                                before.source == after.source
                                    && Arc::ptr_eq(&before.parse, &after.parse)
                            })
                })
                .map(|archive| archive.parsed_sources.clone())
                .unwrap_or_else(|| Arc::from(parsed.to_vec())),
            files,
            bodies,
            exposure,
            revisions: revisions.clone(),
            overlaid_bodies,
        })
    }

    pub fn source(&self, id: SourceId) -> Option<&DefinitionSourceUnit> {
        self.files.get(&id)
    }
    pub fn body(&self, item: HirItemId) -> Option<&AstBodyRef> {
        self.bodies.get(&item)
    }
    pub fn exposure(&self, item: HirItemId) -> BodyExposure {
        self.exposure.get(&item).copied().unwrap_or_default()
    }
    pub(crate) fn has_retained_bodies(&self) -> bool {
        self.exposure.values().any(|exposure| exposure.retained)
    }
    pub(crate) fn retained_items(&self) -> impl Iterator<Item = HirItemId> + '_ {
        self.exposure
            .iter()
            .filter_map(|(item, exposure)| exposure.retained.then_some(*item))
    }
    /// Native archive access, like `source`; language reflection must still
    /// perform the catalog's visibility and retention checks.
    pub fn body_revision_syntax(
        &self,
        item: HirItemId,
    ) -> Option<&etas_code::syntax::SyntaxBlockRef> {
        self.overlaid_bodies
            .get(&item)
            .map(|revision| &revision.syntax)
    }
    pub fn requires_revision_context(&self) -> bool {
        !self.revisions.is_empty()
    }
    pub(crate) fn source_set(&self) -> &Arc<SourceSet> {
        &self.source_set
    }
    pub(crate) fn parsed_sources(&self) -> &Arc<[ParsedSource]> {
        &self.parsed_sources
    }
    pub(crate) fn revisions(&self) -> &HashMap<Span, Arc<crate::code::draft::NativeBodyRevision>> {
        &self.revisions
    }
    pub(crate) fn overlaid_body(
        &self,
        item: HirItemId,
    ) -> Option<&Arc<crate::code::draft::NativeBodyRevision>> {
        self.overlaid_bodies.get(&item)
    }
}
