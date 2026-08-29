use std::collections::{BTreeMap, BTreeSet};

use etas_effects::{
    ActionEventSource, ActionRef, ActionTraceDomain, Effect, EffectCoverage, EffectRegistry,
    EffectRow, EffectSet, TraceSpecClauseFact,
};
use etas_hir::{HirItem, HirItemId, Visibility};
use etas_package_metadata::{
    ActionArgKind as MetadataActionArgKind, ActionSignature as MetadataActionSignature,
    ActionTrace as MetadataActionTrace, ActionTraceEvent as MetadataActionTraceEvent,
    ActionTraceEventSource as MetadataActionTraceEventSource,
    AnnotationArgMetadata as MetadataAnnotationArg,
    AnnotationFieldMetadata as MetadataAnnotationField, AnnotationMetadata as MetadataAnnotation,
    AnnotationValueKind as MetadataAnnotationValueKind,
    AnnotationValueMetadata as MetadataAnnotationValue,
    CallableSignature as MetadataCallableSignature,
    CallableSpecSatisfaction as MetadataCallableSpecSatisfaction, EffectArg as MetadataEffectArg,
    EffectArgKind as MetadataEffectArgKind, EffectExtension as MetadataEffectExtension,
    EffectMetadata as MetadataEffectMetadata, EffectRef as MetadataEffectRef,
    EffectRow as MetadataEffectRow, EffectSummary as MetadataEffectSummary,
    EffectTag as MetadataEffectTag, EncodedMetadataSection,
    ExternalExport as MetadataExternalExport, ExternalModule as MetadataExternalModule,
    LatentFlowSummary as MetadataLatentFlowSummary, NamedSignature as MetadataNamedSignature,
    PackageIdentity, PackageMetadata, PublicMetadata, PublicSymbol, PublicSymbols,
    SpecBound as MetadataSpecBound, SpecImpl as MetadataSpecImpl, SpecKind as MetadataSpecKind,
    SpecMethod as MetadataSpecMethod, SpecSignature as MetadataSpecSignature,
    ToolSchema as MetadataToolSchema, TraceSpecClause as MetadataTraceSpecClause,
    TraceSpecClauseKind as MetadataTraceSpecClauseKind,
    TraceSpecConformance as MetadataTraceSpecConformance,
    TraceSpecConformanceTarget as MetadataTraceSpecConformanceTarget,
    TraceSpecSummary as MetadataTraceSpecSummary, Type as MetadataType,
    TypeField as MetadataTypeField, TypeKind as MetadataTypeKind,
    TypeSpecSatisfaction as MetadataTypeSpecSatisfaction, Visibility as MetadataVisibility,
    blake3_hash, package_metadata_to_sections,
};
use etas_std::{RequirementSemantics, StdDecl};
use etas_types::{
    AgentSignature, EffectActionSignature, EffectArgRef, EffectRef, EffectRowRef, FlowSignature,
    ItemSignature, PrimitiveType, ResourceHandleType, ToolSignature, TrustWrapper, Type, TypeId,
    TypeStore,
};
use serde_json::{Value, json};

use crate::{AstItemKind, CheckedProject, ModuleOrigin};

use super::{PackageMetadataBuildInput, PackageMetadataError, PackageMetadataHeader};

pub(super) struct ProjectMetadataProjection<'a> {
    checked: &'a CheckedProject,
    hir_items_by_origin: BTreeMap<(u32, u32, u32), HirItemId>,
    canonical_type_paths: BTreeMap<String, Option<Vec<String>>>,
    effect_registry: EffectRegistry,
}

impl<'a> ProjectMetadataProjection<'a> {
    pub(super) fn new(checked: &'a CheckedProject) -> Self {
        let mut hir_items_by_origin = BTreeMap::new();
        for (item, origin) in &checked.source_map.item_sources {
            let span = origin.span();
            hir_items_by_origin
                .insert((span.source.0, span.range.start.0, span.range.end.0), *item);
        }
        Self {
            checked,
            hir_items_by_origin,
            canonical_type_paths: build_canonical_type_paths(checked),
            effect_registry: checked.effect_registry.clone(),
        }
    }

    pub(super) fn build_sections(
        &self,
        input: &PackageMetadataBuildInput,
        header: &PackageMetadataHeader,
    ) -> Result<Vec<EncodedMetadataSection>, PackageMetadataError> {
        let external_modules = self.external_modules();
        let mut public_metadata = self.public_metadata(&external_modules)?;
        let effect_metadata = self.effect_metadata(&public_metadata)?;
        public_metadata.fingerprint = Some(blake3_hash(
            format!(
                "{:?}{:?}",
                public_symbols(&public_metadata),
                effect_metadata
            )
            .as_bytes(),
        ));
        let package_metadata = PackageMetadata {
            version: 1,
            package: PackageIdentity {
                name: header.package_id.clone(),
                version: header.package_version.clone(),
                edition: input.package_edition.clone(),
            },
            dependencies: input.dependencies.clone(),
            external_modules,
            public_metadata,
            effect_metadata,
            tool_bindings: input.tool_bindings.clone(),
            bins: input.bins.clone(),
        };
        Ok(package_metadata_to_sections(&package_metadata))
    }

    fn external_modules(&self) -> Vec<MetadataExternalModule> {
        let mut modules = Vec::new();
        for (module_id, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            let mut exports = module
                .visibility_exports
                .items
                .values()
                .enumerate()
                .map(|(index, export)| MetadataExternalExport {
                    id: index.min(u32::MAX as usize) as u32,
                    name: export.name.clone(),
                    visibility: visibility_name(export.visibility),
                })
                .collect::<Vec<_>>();
            exports.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
            modules.push(MetadataExternalModule {
                package: None,
                id: module_id.0,
                path: module.path.segments.clone(),
                exports,
            });
        }
        modules.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
        modules
    }

    fn public_metadata(
        &self,
        modules: &[MetadataExternalModule],
    ) -> Result<PublicMetadata, PackageMetadataError> {
        let mut metadata = PublicMetadata {
            modules: modules.to_vec(),
            ..Default::default()
        };
        for (_, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            for export in module.visibility_exports.items.values() {
                let mut path = module.path.segments.clone();
                path.push(export.name.clone());
                let visibility = visibility_name(export.visibility);
                if let Ok(item) = self.hir_item_for_export(export) {
                    metadata
                        .annotations
                        .extend(self.annotation_metadata_for_item(path.clone(), item)?);
                }
                match export.item.kind {
                    AstItemKind::Type | AstItemKind::TypeAlias => metadata
                        .types
                        .push(self.type_signature(path, visibility, export)?),
                    AstItemKind::Enum | AstItemKind::Error => {
                        metadata.enums.push(named_signature(path, visibility))
                    }
                    AstItemKind::Effect => metadata.effects.push(named_signature(path, visibility)),
                    AstItemKind::Protocol => {
                        metadata.protocols.push(named_signature(path, visibility))
                    }
                    AstItemKind::Flow
                    | AstItemKind::Agent
                    | AstItemKind::Tool
                    | AstItemKind::TopLevelLet => {
                        let item = self.hir_item_for_export(export)?;
                        let signature =
                            self.checked
                                .types
                                .item_signatures
                                .get(&item)
                                .ok_or_else(|| PackageMetadataError::MissingExportSignature {
                                    path: path.clone(),
                                })?;
                        match signature {
                            ItemSignature::Flow(signature) => metadata
                                .flows
                                .push(self.flow_signature(item, path, visibility, signature)?),
                            ItemSignature::Agent(signature) => metadata
                                .agents
                                .push(self.agent_signature(item, path, visibility, signature)?),
                            ItemSignature::Tool(signature) => {
                                metadata.tool_schemas.push(self.tool_schema(
                                    path.clone(),
                                    item,
                                    signature,
                                )?);
                                metadata
                                    .tools
                                    .push(self.tool_signature(item, path, visibility, signature)?);
                            }
                            ItemSignature::TopLevelLet(_) => {
                                metadata
                                    .values
                                    .push(self.value_signature(path, visibility, signature)?);
                            }
                        }
                    }
                    AstItemKind::Spec => {
                        let item = self.hir_item_for_export(export)?;
                        if self.is_trace_spec_item(item) {
                            metadata.trace_specs.push(named_signature(path, visibility));
                        }
                    }
                    AstItemKind::Impl => {}
                }
            }
        }
        metadata.actions = self.action_signatures()?;
        metadata.spec_signatures = self.spec_signatures()?;
        metadata.spec_impls = self.spec_impls()?;
        metadata.type_spec_satisfactions = self.type_spec_satisfactions()?;
        metadata.callable_spec_satisfactions = self.callable_spec_satisfactions()?;
        metadata.trace_spec_conformances = self.trace_spec_conformances()?;
        metadata.effect_summaries = self.effect_summaries()?;
        metadata.trace_spec_summaries = self.trace_spec_summaries()?;
        metadata.annotations.sort_by(|left, right| {
            left.item
                .cmp(&right.item)
                .then_with(|| left.path.cmp(&right.path))
        });
        Ok(metadata)
    }

    fn is_trace_spec_item(&self, item: HirItemId) -> bool {
        matches!(
            self.checked.hir.items.get(item),
            Some(HirItem::Spec(decl))
                if matches!(decl.kind, etas_hir::HirSpecKind::TraceSpec)
        )
    }

    fn annotation_metadata_for_item(
        &self,
        item_path: Vec<String>,
        item: HirItemId,
    ) -> Result<Vec<MetadataAnnotation>, PackageMetadataError> {
        let Some(annotations) = self.checked.hir.item_annotations.get(&item) else {
            return Ok(Vec::new());
        };
        annotations
            .iter()
            .map(|annotation| {
                Ok(MetadataAnnotation {
                    item: item_path.clone(),
                    path: path_segments(&annotation.path),
                    args: annotation
                        .args
                        .iter()
                        .map(|arg| self.annotation_arg_metadata(arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect()
    }

    fn annotation_arg_metadata(
        &self,
        arg: &etas_hir::HirAnnotationArg,
    ) -> Result<MetadataAnnotationArg, PackageMetadataError> {
        match arg {
            etas_hir::HirAnnotationArg::Positional { value, .. } => Ok(MetadataAnnotationArg {
                name: String::new(),
                value: self.annotation_value_metadata(*value)?,
            }),
            etas_hir::HirAnnotationArg::Named { name, value, .. } => Ok(MetadataAnnotationArg {
                name: name.clone(),
                value: self.annotation_value_metadata(*value)?,
            }),
        }
    }

    fn annotation_value_metadata(
        &self,
        expr: etas_hir::HirExprId,
    ) -> Result<MetadataAnnotationValue, PackageMetadataError> {
        let Some(expr_data) = self.checked.hir.exprs.get(expr) else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation expression is missing from checked HIR".to_owned(),
            });
        };
        match expr_data {
            etas_hir::HirExpr::Literal(literal) => Ok(self.annotation_literal_metadata(literal)),
            etas_hir::HirExpr::Path(path) => Ok(MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Path,
                path: self.annotation_path_metadata(path)?,
                ..Default::default()
            }),
            etas_hir::HirExpr::Array { elems, .. } => {
                self.annotation_sequence_metadata(MetadataAnnotationValueKind::Array, elems)
            }
            etas_hir::HirExpr::List { elems, .. } => {
                self.annotation_sequence_metadata(MetadataAnnotationValueKind::List, elems)
            }
            etas_hir::HirExpr::Set { elems, .. } => {
                self.annotation_sequence_metadata(MetadataAnnotationValueKind::Set, elems)
            }
            etas_hir::HirExpr::Tuple { elems, .. } => {
                self.annotation_sequence_metadata(MetadataAnnotationValueKind::Tuple, elems)
            }
            etas_hir::HirExpr::EmptySequence { .. } => Ok(MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Array,
                ..Default::default()
            }),
            etas_hir::HirExpr::EmptyRecordOrMap { .. } => Ok(MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Record,
                ..Default::default()
            }),
            etas_hir::HirExpr::Record(record) => self.annotation_record_metadata(record),
            etas_hir::HirExpr::Call {
                callee,
                generic_args,
                args,
                ..
            } if generic_args.is_empty() => self.annotation_call_metadata(*callee, args),
            _ => Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation argument cannot be represented as static package metadata"
                    .to_owned(),
            }),
        }
    }

    fn annotation_literal_metadata(
        &self,
        literal: &etas_hir::HirLiteral,
    ) -> MetadataAnnotationValue {
        match literal {
            etas_hir::HirLiteral::Bool { value, .. } => MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Bool,
                value: value.to_string(),
                ..Default::default()
            },
            etas_hir::HirLiteral::Int { text, .. } => MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Int,
                value: text.clone(),
                ..Default::default()
            },
            etas_hir::HirLiteral::Float { text, .. } => MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Float,
                value: text.clone(),
                ..Default::default()
            },
            etas_hir::HirLiteral::String { value, .. } => MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::String,
                value: value.clone(),
                ..Default::default()
            },
            etas_hir::HirLiteral::Char { value, .. } => MetadataAnnotationValue {
                kind: MetadataAnnotationValueKind::Char,
                value: value.to_string(),
                ..Default::default()
            },
        }
    }

    fn annotation_sequence_metadata(
        &self,
        kind: MetadataAnnotationValueKind,
        elems: &[etas_hir::HirExprId],
    ) -> Result<MetadataAnnotationValue, PackageMetadataError> {
        Ok(MetadataAnnotationValue {
            kind,
            elements: elems
                .iter()
                .copied()
                .map(|elem| self.annotation_value_metadata(elem))
                .collect::<Result<Vec<_>, _>>()?,
            ..Default::default()
        })
    }

    fn annotation_record_metadata(
        &self,
        record: &etas_hir::HirRecordExpr,
    ) -> Result<MetadataAnnotationValue, PackageMetadataError> {
        if record.path.is_some() || !record.generic_args.is_empty() {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation record metadata must be an anonymous static record".to_owned(),
            });
        }
        Ok(MetadataAnnotationValue {
            kind: MetadataAnnotationValueKind::Record,
            fields: record
                .fields
                .iter()
                .map(|field| self.annotation_record_field_metadata(field))
                .collect::<Result<Vec<_>, _>>()?,
            ..Default::default()
        })
    }

    fn annotation_record_field_metadata(
        &self,
        field: &etas_hir::HirFieldInit,
    ) -> Result<MetadataAnnotationField, PackageMetadataError> {
        match field {
            etas_hir::HirFieldInit::Named { name, value, .. } => Ok(MetadataAnnotationField {
                name: name.clone(),
                value: self.annotation_value_metadata(*value)?,
            }),
            etas_hir::HirFieldInit::Shorthand {
                name, resolution, ..
            } => {
                let path =
                    self.annotation_resolution_path(resolution, name, "annotation record field")?;
                Ok(MetadataAnnotationField {
                    name: name.clone(),
                    value: MetadataAnnotationValue {
                        kind: MetadataAnnotationValueKind::Path,
                        path,
                        ..Default::default()
                    },
                })
            }
        }
    }

    fn annotation_call_metadata(
        &self,
        callee: etas_hir::HirExprId,
        args: &[etas_hir::HirArg],
    ) -> Result<MetadataAnnotationValue, PackageMetadataError> {
        let Some(etas_hir::HirExpr::Path(path)) = self.checked.hir.exprs.get(callee) else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation constructor metadata requires a static constructor path"
                    .to_owned(),
            });
        };
        if let Ok((name, canonical_path)) = self.annotation_limit_constructor_metadata(path) {
            return self.annotation_call_metadata_with_kind(
                MetadataAnnotationValueKind::Limit,
                name,
                canonical_path,
                args,
            );
        }
        let (name, canonical_path) = self.annotation_trace_constructor_metadata(path)?;
        self.annotation_call_metadata_with_kind(
            MetadataAnnotationValueKind::Constructor,
            name,
            canonical_path,
            args,
        )
    }

    fn annotation_call_metadata_with_kind(
        &self,
        kind: MetadataAnnotationValueKind,
        name: String,
        canonical_path: Vec<String>,
        args: &[etas_hir::HirArg],
    ) -> Result<MetadataAnnotationValue, PackageMetadataError> {
        let mut elements = Vec::new();
        let mut fields = Vec::new();
        for arg in args {
            match arg {
                etas_hir::HirArg::Positional(value) => {
                    elements.push(self.annotation_value_metadata(*value)?);
                }
                etas_hir::HirArg::Named { name, value, .. } => {
                    fields.push(MetadataAnnotationField {
                        name: name.clone(),
                        value: self.annotation_value_metadata(*value)?,
                    });
                }
            }
        }
        Ok(MetadataAnnotationValue {
            kind,
            value: name,
            path: canonical_path,
            elements,
            fields,
        })
    }

    fn annotation_limit_constructor_metadata(
        &self,
        path: &etas_hir::ResolvedPath,
    ) -> Result<(String, Vec<String>), PackageMetadataError> {
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation limit constructor must resolve to a std requirement".to_owned(),
            });
        };
        let Some(symbol) = self.checked.hir.symbols.get(symbol) else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation limit constructor symbol is missing from checked HIR"
                    .to_owned(),
            });
        };
        let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation limit constructor must be imported from std".to_owned(),
            });
        };
        let Some(std_symbol) = self.checked.std_registry.lookup_qualified(path) else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation limit constructor is not present in the std registry"
                    .to_owned(),
            });
        };
        let StdDecl::Requirement(requirement) = &std_symbol.decl else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation limit constructor is not a std requirement".to_owned(),
            });
        };
        let RequirementSemantics::Limit(_) = requirement.semantics else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "std requirement is not a runtime limit".to_owned(),
            });
        };
        Ok((std_symbol.name.clone(), std_symbol.qualified_path.clone()))
    }

    fn annotation_trace_constructor_metadata(
        &self,
        path: &etas_hir::ResolvedPath,
    ) -> Result<(String, Vec<String>), PackageMetadataError> {
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor must resolve to a checked std symbol"
                    .to_owned(),
            });
        };
        let Some(symbol) = self.checked.hir.symbols.get(symbol) else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor symbol is missing from checked HIR"
                    .to_owned(),
            });
        };
        let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor must reference std.runtime.trace".to_owned(),
            });
        };
        let Some(std_symbol) = self.checked.std_registry.lookup_qualified(path) else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor is not present in the std registry"
                    .to_owned(),
            });
        };
        if std_symbol.qualified_path.as_slice()
            != ["std", "runtime", "trace", std_symbol.name.as_str()]
        {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor must be in std.runtime.trace".to_owned(),
            });
        }
        let StdDecl::Flow(flow) = &std_symbol.decl else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor must be a pure std flow".to_owned(),
            });
        };
        if !flow.public_effects.is_empty() || !flow.requested_actions.is_empty() {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: "annotation trace constructor must be pure".to_owned(),
            });
        }
        Ok((flow.name.clone(), std_symbol.qualified_path.clone()))
    }

    fn annotation_path_metadata(
        &self,
        path: &etas_hir::ResolvedPath,
    ) -> Result<Vec<String>, PackageMetadataError> {
        match &path.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => self
                .symbol_canonical_path(*symbol)
                .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                    reason: format!(
                        "annotation path `{}` has no canonical metadata identity",
                        path_segments(path).join(".")
                    ),
                }),
            etas_hir::ResolveResult::Unresolved | etas_hir::ResolveResult::PartiallyResolved(_) => {
                Err(PackageMetadataError::InvalidMetadataType {
                    reason: format!(
                        "annotation path `{}` has no resolved canonical metadata identity",
                        path_segments(path).join(".")
                    ),
                })
            }
            etas_hir::ResolveResult::Ambiguous(_) => {
                Err(PackageMetadataError::InvalidMetadataType {
                    reason: format!(
                        "ambiguous annotation path `{}` cannot be emitted in metadata",
                        path_segments(path).join(".")
                    ),
                })
            }
        }
    }

    fn annotation_resolution_path(
        &self,
        resolution: &etas_hir::ResolveResult,
        fallback: &str,
        context: &str,
    ) -> Result<Vec<String>, PackageMetadataError> {
        match resolution {
            etas_hir::ResolveResult::Resolved(symbol) => self
                .symbol_canonical_path(*symbol)
                .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                    reason: format!("{context} `{fallback}` has no canonical metadata identity"),
                }),
            etas_hir::ResolveResult::Unresolved
            | etas_hir::ResolveResult::PartiallyResolved(_)
            | etas_hir::ResolveResult::Ambiguous(_) => {
                Err(PackageMetadataError::InvalidMetadataType {
                    reason: format!("{context} `{fallback}` is not resolved"),
                })
            }
        }
    }

    fn action_signatures(&self) -> Result<Vec<MetadataActionSignature>, PackageMetadataError> {
        let mut actions = Vec::new();
        for (symbol, signature) in &self.checked.types.action_signatures {
            let Some(path) = self.action_path(*symbol) else {
                continue;
            };
            let visibility = self
                .checked
                .hir
                .symbols
                .get(*symbol)
                .map(|symbol| visibility_name(symbol.visibility))
                .unwrap_or(MetadataVisibility::Private);
            actions.push(self.action_signature(path, visibility, signature)?);
        }
        actions.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(actions)
    }

    fn type_signature(
        &self,
        path: Vec<String>,
        visibility: MetadataVisibility,
        export: &crate::ExportedItem,
    ) -> Result<MetadataNamedSignature, PackageMetadataError> {
        let item = self.hir_item_for_export(export)?;
        let symbol = match self.checked.hir.items.get(item) {
            Some(HirItem::Type(decl)) => decl.symbol,
            Some(HirItem::TypeAlias(decl)) => decl.symbol,
            _ => return Err(PackageMetadataError::MissingExportSignature { path }),
        };
        let ty = match self.checked.types.symbol_types.get(&symbol) {
            Some(etas_types::SymbolTypeFact::Type { constructor }) => {
                let mut ty = self.metadata_type(TypeId(constructor.0))?;
                canonicalize_declared_type_path(&mut ty, &path);
                Some(ty)
            }
            Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) => {
                let mut ty = self.metadata_type(TypeId(constructor.0))?;
                canonicalize_declared_type_path(&mut ty, &path);
                Some(ty)
            }
            Some(etas_types::SymbolTypeFact::TypeAlias { target, .. }) => Some(
                metadata_alias_type(&path, *target, &self.checked.type_store)?,
            ),
            _ => None,
        };
        Ok(MetadataNamedSignature {
            path,
            visibility,
            ty,
        })
    }

    fn action_signature(
        &self,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &EffectActionSignature,
    ) -> Result<MetadataActionSignature, PackageMetadataError> {
        let generic_names = signature
            .generic_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        Ok(MetadataActionSignature {
            path,
            generic_params: self.callable_generic_params_metadata(&signature.generic_params)?,
            params: signature
                .params
                .iter()
                .map(|ty| self.metadata_type_in_generic_scope(*ty, &generic_names))
                .collect::<Result<Vec<_>, _>>()?,
            effect_args: signature
                .effect_args
                .iter()
                .map(metadata_action_arg_kind)
                .collect(),
            selector_param_names: signature.selector_param_names.clone(),
            selector_defaults: signature
                .selector_defaults
                .iter()
                .map(|arg| {
                    arg.as_ref()
                        .map(|arg| self.metadata_effect_arg_in_generic_scope(arg, &generic_names))
                        .transpose()
                })
                .collect::<Result<Vec<_>, _>>()?,
            output: Some(self.metadata_type_in_generic_scope(signature.output, &generic_names)?),
            returns_never: signature.returns_never,
            visibility,
        })
    }

    fn checked_spec_bound_metadata(
        &self,
        bound: &etas_types::CheckedSpecBound,
        generic_names: &BTreeSet<String>,
    ) -> Result<MetadataSpecBound, PackageMetadataError> {
        let spec = match &bound.spec {
            etas_types::CheckedSpecRef::Source(symbol) => self
                .symbol_canonical_path(*symbol)
                .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                    reason: "action generic spec bound has no canonical metadata path".to_owned(),
                })?,
            etas_types::CheckedSpecRef::Std(path) => path.clone(),
        };
        Ok(MetadataSpecBound {
            spec,
            args: bound
                .args
                .iter()
                .map(|arg| self.metadata_type_in_generic_scope(*arg, generic_names))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn callable_generic_params_metadata(
        &self,
        params: &[etas_types::CallableGenericParam],
    ) -> Result<Vec<etas_package_metadata::GenericParam>, PackageMetadataError> {
        let generic_names = params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        params
            .iter()
            .map(|param| {
                Ok(etas_package_metadata::GenericParam {
                    name: param.name.clone(),
                    kind: match param.kind {
                        etas_types::CallableGenericParamKind::Type => {
                            etas_package_metadata::GenericParamKind::Type
                        }
                        etas_types::CallableGenericParamKind::Effect => {
                            etas_package_metadata::GenericParamKind::Effect
                        }
                    },
                    bounds: param
                        .bounds
                        .iter()
                        .map(|bound| self.checked_spec_bound_metadata(bound, &generic_names))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect()
    }

    fn spec_signatures(&self) -> Result<Vec<MetadataSpecSignature>, PackageMetadataError> {
        let mut signatures = Vec::new();
        for (_, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            for export in module.visibility_exports.items.values() {
                if !matches!(export.item.kind, AstItemKind::Spec) {
                    continue;
                }
                let item = self.hir_item_for_export(export)?;
                let Some(HirItem::Spec(_)) = self.checked.hir.items.get(item) else {
                    continue;
                };
                let mut path = module.path.segments.clone();
                path.push(export.name.clone());
                let Some(symbol) = self.spec_item_symbol(item) else {
                    return Err(PackageMetadataError::MissingExportSignature { path });
                };
                let signature =
                    self.checked
                        .types
                        .spec_signatures
                        .get(&symbol)
                        .ok_or_else(|| PackageMetadataError::MissingExportSignature {
                            path: path.clone(),
                        })?;
                signatures.push(self.spec_signature(
                    path,
                    visibility_name(export.visibility),
                    signature,
                )?);
            }
        }
        signatures.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(signatures)
    }

    fn spec_item_symbol(&self, item: HirItemId) -> Option<etas_hir::SymbolId> {
        match self.checked.hir.items.get(item)? {
            HirItem::Spec(spec) => Some(spec.symbol),
            _ => None,
        }
    }

    fn spec_signature(
        &self,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &etas_types::SpecSignature,
    ) -> Result<MetadataSpecSignature, PackageMetadataError> {
        Ok(MetadataSpecSignature {
            path: path.clone(),
            visibility,
            kind: match signature.kind {
                etas_types::SpecKind::TypeSpec => MetadataSpecKind::Type,
                etas_types::SpecKind::CallableSpec => MetadataSpecKind::Callable,
                etas_types::SpecKind::TraceSpec => MetadataSpecKind::Trace,
            },
            param_names: signature.param_names.clone(),
            callable: signature
                .callable
                .as_ref()
                .map(|callable| {
                    self.metadata_callable_signature_from_callable(
                        path.clone(),
                        visibility,
                        callable,
                    )
                })
                .transpose()?,
            methods: signature
                .methods
                .iter()
                .map(|method| self.spec_method_metadata(method, visibility))
                .collect::<Result<Vec<_>, _>>()?,
            super_specs: signature
                .super_specs
                .iter()
                .map(|bound| self.spec_bound_metadata(bound))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn spec_impls(&self) -> Result<Vec<MetadataSpecImpl>, PackageMetadataError> {
        self.checked
            .types
            .spec_impls
            .iter()
            .map(|implementation| {
                let spec = self
                    .symbol_canonical_path(implementation.spec_symbol)
                    .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                        reason: "spec impl target has no canonical metadata path".to_owned(),
                    })?;
                Ok(MetadataSpecImpl {
                    self_type: self.metadata_type(implementation.self_type)?,
                    spec,
                    args: implementation
                        .args
                        .iter()
                        .map(|arg| self.metadata_type(*arg))
                        .collect::<Result<Vec<_>, _>>()?,
                    methods: implementation
                        .methods
                        .iter()
                        .map(|method| method.name.clone())
                        .collect(),
                })
            })
            .collect()
    }

    fn type_spec_satisfactions(
        &self,
    ) -> Result<Vec<MetadataTypeSpecSatisfaction>, PackageMetadataError> {
        self.checked
            .types
            .type_spec_satisfactions
            .iter()
            .map(|fact| {
                let spec = self
                    .symbol_canonical_path(fact.spec_symbol)
                    .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                        reason: "type spec satisfaction target has no canonical metadata path"
                            .to_owned(),
                    })?;
                Ok(MetadataTypeSpecSatisfaction {
                    self_type: self.metadata_type(fact.self_type)?,
                    spec,
                    args: fact
                        .args
                        .iter()
                        .map(|arg| self.metadata_type(*arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect()
    }

    fn callable_spec_satisfactions(
        &self,
    ) -> Result<Vec<MetadataCallableSpecSatisfaction>, PackageMetadataError> {
        self.checked
            .types
            .callable_spec_satisfactions
            .iter()
            .map(|fact| {
                let item = self.item_path(fact.item).ok_or_else(|| {
                    PackageMetadataError::InvalidMetadataType {
                        reason: "callable spec satisfaction item has no canonical metadata path"
                            .to_owned(),
                    }
                })?;
                let spec = self
                    .symbol_canonical_path(fact.spec_symbol)
                    .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                        reason: "callable spec satisfaction target has no canonical metadata path"
                            .to_owned(),
                    })?;
                Ok(MetadataCallableSpecSatisfaction {
                    item,
                    spec,
                    args: fact
                        .args
                        .iter()
                        .map(|arg| self.metadata_type(*arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect()
    }

    fn trace_spec_conformances(
        &self,
    ) -> Result<Vec<MetadataTraceSpecConformance>, PackageMetadataError> {
        self.checked
            .types
            .trace_spec_conformances
            .iter()
            .map(|fact| {
                let item = self.item_path(fact.item).ok_or_else(|| {
                    PackageMetadataError::InvalidMetadataType {
                        reason: "trace spec conformance item has no canonical metadata path"
                            .to_owned(),
                    }
                })?;
                Ok(MetadataTraceSpecConformance {
                    item,
                    target: match &fact.target {
                        etas_types::TraceSpecConformanceTarget::Inline => {
                            MetadataTraceSpecConformanceTarget::Inline
                        }
                        etas_types::TraceSpecConformanceTarget::Named { spec_symbol, args } => {
                            MetadataTraceSpecConformanceTarget::Named {
                                spec: self.symbol_canonical_path(*spec_symbol).ok_or_else(
                                    || PackageMetadataError::InvalidMetadataType {
                                        reason: "trace spec conformance target has no canonical metadata path"
                                            .to_owned(),
                                    },
                                )?,
                                args: args
                                    .iter()
                                    .map(|arg| self.metadata_type(*arg))
                                    .collect::<Result<Vec<_>, _>>()?,
                            }
                        }
                    },
                })
            })
            .collect()
    }

    fn spec_method_metadata(
        &self,
        method: &etas_types::SpecMethodFact,
        visibility: MetadataVisibility,
    ) -> Result<MetadataSpecMethod, PackageMetadataError> {
        let Some(symbol) = method.source_symbol() else {
            return Err(PackageMetadataError::InvalidMetadataType {
                reason: format!(
                    "external spec method `{}` cannot be projected as source package metadata",
                    method.name
                ),
            });
        };
        let path = self.symbol_canonical_path(symbol).ok_or_else(|| {
            PackageMetadataError::InvalidMetadataType {
                reason: format!(
                    "spec method `{}` has no canonical metadata path",
                    method.name
                ),
            }
        })?;
        let callable = self
            .checked
            .types
            .symbol_types
            .get(&symbol)
            .and_then(|fact| match fact {
                etas_types::SymbolTypeFact::Flow { signature } => Some(signature),
                _ => None,
            })
            .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                reason: format!(
                    "spec method `{}` is missing checked flow signature",
                    method.name
                ),
            })?;
        Ok(MetadataSpecMethod {
            name: method.name.clone(),
            path: path.clone(),
            signature: Some(
                self.metadata_callable_signature_from_callable(path, visibility, callable)?,
            ),
        })
    }

    fn spec_bound_metadata(
        &self,
        bound: &etas_types::SpecSuperBoundFact,
    ) -> Result<MetadataSpecBound, PackageMetadataError> {
        let spec = self
            .symbol_canonical_path(bound.super_spec_symbol)
            .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                reason: "spec super bound has no canonical metadata path".to_owned(),
            })?;
        Ok(MetadataSpecBound {
            spec,
            args: bound
                .args
                .iter()
                .map(|arg| self.metadata_type(*arg))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn metadata_callable_signature_from_callable(
        &self,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &etas_types::CallableSignature,
    ) -> Result<MetadataCallableSignature, PackageMetadataError> {
        let generic_names = signature
            .generic_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        Ok(MetadataCallableSignature {
            path,
            generic_params: self.callable_generic_params_metadata(&signature.generic_params)?,
            param_names: Vec::new(),
            input: signature
                .params
                .iter()
                .map(|ty| self.metadata_type_in_generic_scope(*ty, &generic_names))
                .collect::<Result<Vec<_>, _>>()?,
            output: Some(self.metadata_type_in_generic_scope(signature.output, &generic_names)?),
            effects: signature
                .effects
                .as_ref()
                .map(|row| self.metadata_effect_row_in_generic_scope(row, &generic_names))
                .transpose()?,
            visibility,
        })
    }

    fn value_signature(
        &self,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &ItemSignature,
    ) -> Result<MetadataNamedSignature, PackageMetadataError> {
        let ItemSignature::TopLevelLet(signature) = signature else {
            return Err(PackageMetadataError::MissingExportSignature { path });
        };
        Ok(MetadataNamedSignature {
            path,
            visibility,
            ty: Some(self.metadata_type(signature.ty)?),
        })
    }

    fn metadata_type(&self, ty: TypeId) -> Result<MetadataType, PackageMetadataError> {
        let mut metadata = metadata_type(ty, &self.checked.type_store)?;
        self.canonicalize_metadata_type(&mut metadata)?;
        Ok(metadata)
    }

    fn metadata_type_in_generic_scope(
        &self,
        ty: TypeId,
        generic_names: &BTreeSet<String>,
    ) -> Result<MetadataType, PackageMetadataError> {
        let mut metadata = metadata_type(ty, &self.checked.type_store)?;
        normalize_generic_type(&mut metadata, generic_names);
        self.canonicalize_metadata_type(&mut metadata)?;
        Ok(metadata)
    }

    fn metadata_effect_row_in_generic_scope(
        &self,
        row: &EffectRowRef,
        generic_names: &BTreeSet<String>,
    ) -> Result<MetadataEffectRow, PackageMetadataError> {
        let mut metadata = metadata_effect_row_ref(row, &self.checked.type_store)?;
        normalize_generic_effect_row(&mut metadata, generic_names);
        self.canonicalize_effect_row_types(&mut metadata)?;
        Ok(metadata)
    }

    fn metadata_effect_arg_in_generic_scope(
        &self,
        arg: &EffectArgRef,
        generic_names: &BTreeSet<String>,
    ) -> Result<MetadataEffectArg, PackageMetadataError> {
        let mut metadata = metadata_effect_arg(arg, &self.checked.type_store)?;
        if let Some(ty) = &mut metadata.ty {
            normalize_generic_type(ty, generic_names);
            self.canonicalize_metadata_type(ty)?;
        }
        Ok(metadata)
    }

    fn canonicalize_metadata_type(
        &self,
        metadata: &mut MetadataType,
    ) -> Result<(), PackageMetadataError> {
        if matches!(
            metadata.kind,
            MetadataTypeKind::Named | MetadataTypeKind::Nominal | MetadataTypeKind::Applied
        ) && metadata.path.len() == 1
            && let Some(resolved) = self.canonical_type_paths.get(&metadata.path[0])
        {
            let Some(path) = resolved else {
                return Err(PackageMetadataError::InvalidMetadataType {
                    reason: format!(
                        "ambiguous unqualified type path `{}` in package metadata",
                        metadata.path[0]
                    ),
                });
            };
            metadata.path = path.clone();
        }
        for child in &mut metadata.children {
            self.canonicalize_metadata_type(child)?;
        }
        for field in &mut metadata.fields {
            self.canonicalize_metadata_type(&mut field.ty)?;
        }
        if let Some(effects) = &mut metadata.effects {
            self.canonicalize_effect_row_types(effects)?;
        }
        if let Some(effects) = &mut metadata.produced_effects {
            self.canonicalize_effect_row_types(effects)?;
        }
        Ok(())
    }

    fn canonicalize_effect_row_types(
        &self,
        row: &mut MetadataEffectRow,
    ) -> Result<(), PackageMetadataError> {
        for effect in &mut row.effects {
            self.canonicalize_effect_ref_path(effect)?;
            for arg in &mut effect.args {
                if let Some(ty) = &mut arg.ty {
                    self.canonicalize_metadata_type(ty)?;
                }
            }
        }
        Ok(())
    }

    fn canonicalize_effect_ref_path(
        &self,
        effect: &mut MetadataEffectRef,
    ) -> Result<(), PackageMetadataError> {
        if effect.path == ["Error"] {
            return Ok(());
        }
        let name = effect.path.join(".");
        if let Some(action) = self.effect_registry.action_by_name(&name) {
            effect.path = self.action_metadata_path(&action)?;
            return Ok(());
        }
        if let Some(tag) = self.effect_registry.tag_by_name(&name) {
            effect.path = self.effect_tag_metadata_path(tag)?;
            return Ok(());
        }
        Err(PackageMetadataError::InvalidMetadataType {
            reason: format!(
                "effect reference `{}` cannot be resolved to a checked effect tag or action",
                name
            ),
        })
    }

    fn effect_summaries(&self) -> Result<Vec<MetadataEffectSummary>, PackageMetadataError> {
        let mut summaries = Vec::new();
        for (item, summary) in &self.checked.effects.item_effects {
            let Some(path) = self.item_path(*item) else {
                continue;
            };
            let type_generic_names = self
                .checked
                .types
                .item_signatures
                .get(item)
                .map(callable_type_generic_names)
                .unwrap_or_default();
            let effect_generic_names = self
                .checked
                .types
                .item_signatures
                .get(item)
                .map(callable_effect_generic_names)
                .unwrap_or_default();
            let public_effects = match self.public_contract_for_item(*item) {
                Some(contract) => self.effect_row_metadata(
                    &contract.public_row,
                    &type_generic_names,
                    &effect_generic_names,
                )?,
                None => self.effect_row_metadata(
                    &summary.escaping_effects,
                    &type_generic_names,
                    &effect_generic_names,
                )?,
            };
            summaries.push(MetadataEffectSummary {
                item: path,
                public_effects,
                requested_actions: self.effect_row_metadata(
                    &summary.requested_actions,
                    &type_generic_names,
                    &effect_generic_names,
                )?,
                handled_requested_actions: self.handled_requested_action_row(
                    summary,
                    &type_generic_names,
                    &effect_generic_names,
                )?,
                latent_flows: self.latent_flow_summaries(
                    *item,
                    &type_generic_names,
                    &effect_generic_names,
                )?,
                action_trace: self
                    .action_trace_metadata(&summary.action_trace, &type_generic_names)?,
            });
        }
        summaries.sort_by(|left, right| left.item.cmp(&right.item));
        Ok(summaries)
    }

    fn trace_spec_summaries(&self) -> Result<Vec<MetadataTraceSpecSummary>, PackageMetadataError> {
        let mut summaries = Vec::new();
        for (item, facts) in &self.checked.effects.trace_specs.items {
            let Some(HirItem::Spec(spec)) = self.checked.hir.items.get(*item) else {
                continue;
            };
            if !matches!(spec.kind, etas_hir::HirSpecKind::TraceSpec) {
                continue;
            }
            let Some(path) = self.item_path(*item) else {
                continue;
            };
            let mut clauses = Vec::new();
            for fact in facts {
                if let Some(clause) = self.trace_spec_clause_metadata(fact)? {
                    clauses.push(clause);
                }
            }
            summaries.push(MetadataTraceSpecSummary {
                trace_spec: path,
                clauses,
            });
        }
        summaries.sort_by(|left, right| left.trace_spec.cmp(&right.trace_spec));
        Ok(summaries)
    }

    fn trace_spec_clause_metadata(
        &self,
        fact: &TraceSpecClauseFact,
    ) -> Result<Option<MetadataTraceSpecClause>, PackageMetadataError> {
        Ok(match fact {
            TraceSpecClauseFact::Allow { pattern, .. } => Some(MetadataTraceSpecClause {
                kind: MetadataTraceSpecClauseKind::Allow,
                pattern: Some(self.metadata_effect_row(pattern)?),
                guard: None,
                target: None,
                obligation: None,
            }),
            TraceSpecClauseFact::Deny { pattern, .. } => Some(MetadataTraceSpecClause {
                kind: MetadataTraceSpecClauseKind::Deny,
                pattern: Some(self.metadata_effect_row(pattern)?),
                guard: None,
                target: None,
                obligation: None,
            }),
            TraceSpecClauseFact::RequireBefore { guard, target, .. } => {
                Some(MetadataTraceSpecClause {
                    kind: MetadataTraceSpecClauseKind::RequireBefore,
                    pattern: None,
                    guard: Some(self.metadata_effect_row(guard)?),
                    target: Some(self.metadata_effect_row(target)?),
                    obligation: None,
                })
            }
            TraceSpecClauseFact::RequireAfter {
                target, obligation, ..
            } => Some(MetadataTraceSpecClause {
                kind: MetadataTraceSpecClauseKind::RequireAfter,
                pattern: None,
                guard: None,
                target: Some(self.metadata_effect_row(target)?),
                obligation: Some(self.metadata_effect_row(obligation)?),
            }),
            TraceSpecClauseFact::PendingTypedTraceSpecModel
            | TraceSpecClauseFact::TraceSpecReference { .. }
            | TraceSpecClauseFact::Limit { .. } => None,
        })
    }

    fn metadata_effect_row(
        &self,
        row: &EffectRow,
    ) -> Result<MetadataEffectRow, PackageMetadataError> {
        Ok(MetadataEffectRow {
            effects: row
                .effects
                .iter()
                .map(|effect| self.metadata_effect(effect))
                .collect::<Result<Vec<_>, _>>()?,
            tail: self.effect_row_tail(row, &BTreeSet::new())?,
        })
    }

    fn metadata_effect(&self, effect: &Effect) -> Result<MetadataEffectRef, PackageMetadataError> {
        match effect {
            Effect::Tag(tag) => Ok(MetadataEffectRef {
                path: self.effect_tag_metadata_path(*tag)?,
                args: Vec::new(),
            }),
            Effect::Action(action) => Ok(MetadataEffectRef {
                path: self.action_metadata_path(action)?,
                args: Vec::new(),
            }),
            Effect::AppliedAction(action) => Ok(MetadataEffectRef {
                path: self.action_metadata_path(&action.action)?,
                args: action
                    .args
                    .iter()
                    .map(|arg| metadata_effect_arg(arg, &self.checked.type_store))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Effect::Applied { tag, args } => Ok(MetadataEffectRef {
                path: self.effect_tag_metadata_path(*tag)?,
                args: args
                    .iter()
                    .map(|arg| {
                        Ok(MetadataEffectArg {
                            kind: MetadataEffectArgKind::Type,
                            ty: Some(metadata_type(*arg, &self.checked.type_store)?),
                            path: Vec::new(),
                            value: String::new(),
                        })
                    })
                    .collect::<Result<Vec<_>, PackageMetadataError>>()?,
            }),
            Effect::Error(ty) => Ok(MetadataEffectRef {
                path: vec!["Error".to_owned()],
                args: vec![MetadataEffectArg {
                    kind: MetadataEffectArgKind::Type,
                    ty: Some(metadata_type(*ty, &self.checked.type_store)?),
                    path: Vec::new(),
                    value: String::new(),
                }],
            }),
            Effect::Var(var) => Err(PackageMetadataError::UnresolvedEffectTag { id: var.0 }),
        }
    }

    fn effect_tag_metadata_path(
        &self,
        tag: etas_effects::EffectTagId,
    ) -> Result<Vec<String>, PackageMetadataError> {
        self.effect_tag_path(tag)
            .or_else(|| {
                self.effect_registry
                    .tag_name(tag)
                    .map(|name| name.split('.').map(str::to_owned).collect())
            })
            .ok_or(PackageMetadataError::UnresolvedEffectTag { id: tag.0 })
    }

    fn handled_requested_action_row(
        &self,
        summary: &etas_effects::EffectSummary,
        type_generic_names: &BTreeSet<String>,
        effect_generic_names: &BTreeSet<String>,
    ) -> Result<MetadataEffectRow, PackageMetadataError> {
        let coverage = EffectCoverage {
            registry: &self.effect_registry,
            types: &self.checked.type_store,
        };
        let effects = summary
            .requested_actions
            .effects
            .iter()
            .filter(|effect| {
                if !effect.is_action() {
                    return false;
                }
                let requested = EffectRow::closed(EffectSet::one((*effect).clone()));
                coverage.row_covers(&summary.default_actions, &requested)
                    || coverage.row_covers(&summary.handled_actions, &requested)
            })
            .map(|effect| self.summary_effect_ref_metadata(effect, type_generic_names))
            .collect::<Result<Vec<_>, _>>()?;
        let default_tail = self.effect_row_tail(&summary.default_actions, effect_generic_names)?;
        let handled_tail = self.effect_row_tail(&summary.handled_actions, effect_generic_names)?;
        let tail = match (default_tail, handled_tail) {
            (Some(default), Some(handled)) if default != handled => {
                return Err(PackageMetadataError::InvalidMetadataType {
                    reason: format!(
                        "handled action rows use incompatible effect parameters `{default}` and `{handled}`"
                    ),
                });
            }
            (Some(tail), _) | (_, Some(tail)) => Some(tail),
            (None, None) => None,
        };
        Ok(MetadataEffectRow { effects, tail })
    }

    fn public_contract_for_item(
        &self,
        item: HirItemId,
    ) -> Option<&etas_effects::PublicEffectContract> {
        self.checked
            .effects
            .public_contracts
            .iter()
            .find(|contract| contract.item == item)
    }

    fn latent_flow_summaries(
        &self,
        item: HirItemId,
        type_generic_names: &BTreeSet<String>,
        effect_generic_names: &BTreeSet<String>,
    ) -> Result<Vec<MetadataLatentFlowSummary>, PackageMetadataError> {
        self.checked
            .effects
            .public_contracts
            .iter()
            .filter(|contract| contract.item == item)
            .flat_map(|contract| {
                contract.latent_flows.iter().map(|latent| {
                    let declared_bound = latent
                        .declared_bound
                        .as_ref()
                        .map(|row| {
                            self.effect_row_metadata(row, type_generic_names, effect_generic_names)
                        })
                        .transpose()?
                        .unwrap_or_default();
                    Ok(MetadataLatentFlowSummary {
                        declared_bound,
                        inferred_effects: self.effect_row_metadata(
                            &latent.inferred,
                            type_generic_names,
                            effect_generic_names,
                        )?,
                    })
                })
            })
            .collect()
    }

    fn action_trace_metadata(
        &self,
        trace: &ActionTraceDomain,
        type_generic_names: &BTreeSet<String>,
    ) -> Result<MetadataActionTrace, PackageMetadataError> {
        Ok(match trace {
            ActionTraceDomain::Empty => MetadataActionTrace::Empty,
            ActionTraceDomain::Event(event) => {
                MetadataActionTrace::Event(MetadataActionTraceEvent {
                    action: self.summary_effect_ref_metadata(&event.action, type_generic_names)?,
                    source: match event.source {
                        ActionEventSource::Perform => MetadataActionTraceEventSource::Perform,
                        ActionEventSource::StdIntrinsic => {
                            MetadataActionTraceEventSource::StdIntrinsic
                        }
                        ActionEventSource::AgentCall => MetadataActionTraceEventSource::AgentCall,
                        ActionEventSource::ExternalMetadata => {
                            MetadataActionTraceEventSource::ExternalMetadata
                        }
                        ActionEventSource::Transfer => MetadataActionTraceEventSource::Transfer,
                        ActionEventSource::Unknown => MetadataActionTraceEventSource::Unknown,
                    },
                })
            }
            ActionTraceDomain::ParameterCall { parameter, .. } => {
                MetadataActionTrace::ParameterCall {
                    parameter: parameter.clone(),
                }
            }
            ActionTraceDomain::Seq(children) => MetadataActionTrace::Seq(
                children
                    .iter()
                    .map(|child| self.action_trace_metadata(child, type_generic_names))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ActionTraceDomain::Choice(children) => MetadataActionTrace::Choice(
                children
                    .iter()
                    .map(|child| self.action_trace_metadata(child, type_generic_names))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ActionTraceDomain::Repeat(child) => MetadataActionTrace::Repeat(Box::new(
                self.action_trace_metadata(child, type_generic_names)?,
            )),
            ActionTraceDomain::UnknownOrder(actions) => MetadataActionTrace::UnknownOrder(
                actions
                    .iter()
                    .map(|action| self.summary_effect_ref_metadata(action, type_generic_names))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        })
    }

    fn effect_row_metadata(
        &self,
        row: &EffectRow,
        type_generic_names: &BTreeSet<String>,
        effect_generic_names: &BTreeSet<String>,
    ) -> Result<MetadataEffectRow, PackageMetadataError> {
        Ok(MetadataEffectRow {
            effects: row
                .effects
                .iter()
                .map(|effect| self.summary_effect_ref_metadata(effect, type_generic_names))
                .collect::<Result<Vec<_>, _>>()?,
            tail: self.effect_row_tail(row, effect_generic_names)?,
        })
    }

    fn effect_row_tail(
        &self,
        row: &EffectRow,
        effect_generic_names: &BTreeSet<String>,
    ) -> Result<Option<String>, PackageMetadataError> {
        let Some(open) = row.open else {
            return Ok(None);
        };
        let matches = effect_generic_names
            .iter()
            .filter(|name| etas_effects::effect_var_id_from_name(name) == open)
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [name] => Ok(Some(name.clone())),
            [] => Err(PackageMetadataError::InvalidMetadataType {
                reason: format!(
                    "open effect row variable {:?} does not name a declared effect parameter",
                    open
                ),
            }),
            _ => Err(PackageMetadataError::InvalidMetadataType {
                reason: format!(
                    "open effect row variable {:?} is ambiguous in callable generic parameters",
                    open
                ),
            }),
        }
    }

    fn summary_effect_ref_metadata(
        &self,
        effect: &Effect,
        generic_names: &BTreeSet<String>,
    ) -> Result<MetadataEffectRef, PackageMetadataError> {
        Ok(match effect {
            Effect::Tag(tag) => MetadataEffectRef {
                path: self.effect_tag_metadata_path(*tag)?,
                args: Vec::new(),
            },
            Effect::Action(action) => MetadataEffectRef {
                path: self.action_metadata_path(action)?,
                args: Vec::new(),
            },
            Effect::AppliedAction(action) => MetadataEffectRef {
                path: self.action_metadata_path(&action.action)?,
                args: action
                    .args
                    .iter()
                    .map(|arg| self.metadata_effect_arg_in_generic_scope(arg, generic_names))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Effect::Applied { tag, args } => MetadataEffectRef {
                path: self.effect_tag_metadata_path(*tag)?,
                args: args
                    .iter()
                    .map(|arg| {
                        Ok(MetadataEffectArg {
                            kind: MetadataEffectArgKind::Type,
                            ty: Some(self.metadata_type_in_generic_scope(*arg, generic_names)?),
                            path: Vec::new(),
                            value: String::new(),
                        })
                    })
                    .collect::<Result<Vec<_>, PackageMetadataError>>()?,
            },
            Effect::Var(var) => {
                return Err(PackageMetadataError::UnresolvedEffectTag { id: var.0 });
            }
            Effect::Error(ty) => MetadataEffectRef {
                path: vec!["Error".to_owned()],
                args: vec![MetadataEffectArg {
                    kind: MetadataEffectArgKind::Type,
                    ty: Some(self.metadata_type_in_generic_scope(*ty, generic_names)?),
                    path: Vec::new(),
                    value: String::new(),
                }],
            },
        })
    }

    fn action_metadata_path(
        &self,
        action: &ActionRef,
    ) -> Result<Vec<String>, PackageMetadataError> {
        if let Some(path) = self.action_ref_path(action) {
            return Ok(path);
        }
        let owner = self.effect_tag_metadata_path(action.tag)?;
        let action = self
            .effect_registry
            .action_name(action.tag, action.action)
            .map(str::to_owned)
            .ok_or(PackageMetadataError::UnresolvedEffectAction {
                tag: action.tag.0,
                action: action.action.0,
            })?;
        let mut path = owner;
        path.push(action);
        Ok(path)
    }

    fn effect_tag_path(&self, tag: etas_effects::EffectTagId) -> Option<Vec<String>> {
        if let Some(path) = self.effect_registry.dependency_tag_path(tag) {
            return Some(path.to_vec());
        }
        for (_, item) in self.checked.hir.items.iter() {
            let HirItem::Effect(effect) = item else {
                continue;
            };
            if self.effect_registry.tag_by_symbol(effect.symbol) != Some(tag) {
                continue;
            }
            let symbol = self.checked.hir.symbols.get(effect.symbol)?;
            let module = self.checked.hir.modules_arena.get(symbol.defining_module)?;
            let mut path = module.name.as_ref().map(path_segments)?;
            path.push(local_symbol_name(&symbol.name).to_owned());
            return Some(path);
        }
        None
    }

    fn action_ref_path(&self, action: &ActionRef) -> Option<Vec<String>> {
        if let Some(path) = self.effect_registry.dependency_action_path(action) {
            return Some(path.to_vec());
        }
        for symbol in self.checked.types.action_signatures.keys() {
            let Some(signature) = self.effect_registry.action(*symbol) else {
                continue;
            };
            if signature.owner == action.tag && signature.id == action.action {
                return self.action_path(*symbol);
            }
        }
        None
    }

    fn effect_metadata(
        &self,
        public_metadata: &PublicMetadata,
    ) -> Result<MetadataEffectMetadata, PackageMetadataError> {
        let mut tags = public_metadata
            .effects
            .iter()
            .map(|effect| MetadataEffectTag {
                path: effect.path.clone(),
                runtime_requirement: None,
            })
            .collect::<Vec<_>>();
        tags.sort_by(|left, right| left.path.cmp(&right.path));

        let public_effect_paths = public_metadata
            .effects
            .iter()
            .map(|effect| effect.path.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut extensions = Vec::new();
        for (_, item) in self.checked.hir.items.iter() {
            let HirItem::Effect(effect) = item else {
                continue;
            };
            let Some(child_tag) = self.effect_registry.tag_by_symbol(effect.symbol) else {
                continue;
            };
            let Some(child) = self.effect_tag_path(child_tag) else {
                return Err(PackageMetadataError::UnresolvedEffectTag { id: child_tag.0 });
            };
            if !public_effect_paths.contains(&child) {
                continue;
            }
            let Some(parent_ref) = &effect.extends else {
                continue;
            };
            let parent = self.effect_ref_path(parent_ref)?;
            extensions.push(MetadataEffectExtension { child, parent });
        }
        extensions.sort_by(|left, right| {
            left.child
                .cmp(&right.child)
                .then_with(|| left.parent.cmp(&right.parent))
        });

        Ok(MetadataEffectMetadata { tags, extensions })
    }

    fn flow_signature(
        &self,
        item: HirItemId,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &FlowSignature,
    ) -> Result<MetadataCallableSignature, PackageMetadataError> {
        let generic_names = signature
            .generic_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        Ok(MetadataCallableSignature {
            path,
            generic_params: self.callable_generic_params_metadata(&signature.generic_params)?,
            param_names: self.callable_param_names(item)?,
            input: signature
                .params
                .iter()
                .map(|ty| self.metadata_type_in_generic_scope(*ty, &generic_names))
                .collect::<Result<Vec<_>, _>>()?,
            output: Some(self.metadata_type_in_generic_scope(signature.output, &generic_names)?),
            effects: signature
                .effects
                .as_ref()
                .map(|row| self.metadata_effect_row_in_generic_scope(row, &generic_names))
                .transpose()?,
            visibility,
        })
    }

    fn agent_signature(
        &self,
        item: HirItemId,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &AgentSignature,
    ) -> Result<MetadataCallableSignature, PackageMetadataError> {
        let generic_names = signature
            .generic_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        Ok(MetadataCallableSignature {
            path,
            generic_params: self.callable_generic_params_metadata(&signature.generic_params)?,
            param_names: self.callable_param_names(item)?,
            input: signature
                .params
                .iter()
                .map(|ty| self.metadata_type_in_generic_scope(*ty, &generic_names))
                .collect::<Result<Vec<_>, _>>()?,
            output: Some(self.metadata_type_in_generic_scope(signature.output, &generic_names)?),
            effects: signature
                .effects
                .as_ref()
                .map(|row| self.metadata_effect_row_in_generic_scope(row, &generic_names))
                .transpose()?,
            visibility,
        })
    }

    fn tool_signature(
        &self,
        item: HirItemId,
        path: Vec<String>,
        visibility: MetadataVisibility,
        signature: &ToolSignature,
    ) -> Result<MetadataCallableSignature, PackageMetadataError> {
        let generic_names = signature
            .generic_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<BTreeSet<_>>();
        Ok(MetadataCallableSignature {
            path,
            generic_params: self.callable_generic_params_metadata(&signature.generic_params)?,
            param_names: self.callable_param_names(item)?,
            input: signature
                .params
                .iter()
                .map(|ty| self.metadata_type_in_generic_scope(*ty, &generic_names))
                .collect::<Result<Vec<_>, _>>()?,
            output: Some(self.metadata_type_in_generic_scope(signature.output, &generic_names)?),
            effects: signature
                .effects
                .as_ref()
                .map(|row| self.metadata_effect_row_in_generic_scope(row, &generic_names))
                .transpose()?,
            visibility,
        })
    }

    fn callable_param_names(&self, item: HirItemId) -> Result<Vec<String>, PackageMetadataError> {
        let params = match self.checked.hir.items.get(item) {
            Some(HirItem::Flow(flow)) => &flow.params,
            Some(HirItem::Agent(agent)) => &agent.params,
            Some(HirItem::Tool(tool)) => &tool.params,
            _ => return Ok(Vec::new()),
        };
        params
            .iter()
            .map(|symbol| {
                self.checked
                    .hir
                    .symbols
                    .get(*symbol)
                    .map(|symbol| symbol.name.clone())
                    .ok_or_else(|| PackageMetadataError::MissingExportSignature {
                        path: vec!["<callable-params>".to_owned()],
                    })
            })
            .collect()
    }

    fn tool_schema(
        &self,
        path: Vec<String>,
        item: HirItemId,
        signature: &ToolSignature,
    ) -> Result<MetadataToolSchema, PackageMetadataError> {
        let Some(HirItem::Tool(tool)) = self.checked.hir.items.get(item) else {
            return Err(PackageMetadataError::MissingExportSignature { path });
        };
        if tool.params.len() != signature.params.len() {
            return Err(PackageMetadataError::InvalidToolSchema {
                path,
                reason: "tool parameter count does not match checked signature input".to_owned(),
            });
        }

        let mut properties = serde_json::Map::new();
        let mut required = Vec::with_capacity(tool.params.len());
        for (param, ty) in tool.params.iter().zip(signature.params.iter()) {
            let Some(symbol) = self.checked.hir.symbols.get(*param) else {
                return Err(PackageMetadataError::InvalidToolSchema {
                    path,
                    reason: "tool parameter symbol is missing from checked HIR".to_owned(),
                });
            };
            properties.insert(
                symbol.name.clone(),
                json_schema_for_type(*ty, &self.checked.type_store, 0)?,
            );
            required.push(Value::String(symbol.name.clone()));
        }

        Ok(MetadataToolSchema {
            tool: path,
            schema_json: json!({
                "type": "object",
                "properties": Value::Object(properties),
                "required": required,
                "additionalProperties": false,
            })
            .to_string(),
        })
    }

    fn hir_item_for_export(
        &self,
        export: &crate::ExportedItem,
    ) -> Result<HirItemId, PackageMetadataError> {
        let key = (
            export.item.span.source.0,
            export.item.span.range.start.0,
            export.item.span.range.end.0,
        );
        if let Some(item) = self.hir_items_by_origin.get(&key).copied() {
            return Ok(item);
        }
        for (item, origin) in &self.checked.source_map.item_sources {
            let span = origin.span();
            if span.source != export.item.span.source
                || span.range.start < export.item.span.range.start
                || span.range.end > export.item.span.range.end
            {
                continue;
            }
            if self.item_local_name(*item).as_deref() == Some(export.name.as_str()) {
                return Ok(*item);
            }
        }
        Err(PackageMetadataError::MissingExportSignature {
            path: vec![export.name.clone()],
        })
    }

    fn item_local_name(&self, item: HirItemId) -> Option<String> {
        let symbol = match self.checked.hir.items.get(item)? {
            HirItem::TypeAlias(decl) => decl.symbol,
            HirItem::Type(decl) => decl.symbol,
            HirItem::Enum(decl) => decl.symbol,
            HirItem::Spec(decl) => decl.symbol,
            HirItem::Effect(decl) => decl.symbol,
            HirItem::TopLevelLet(decl) => decl.symbol,
            HirItem::Tool(decl) => decl.symbol,
            HirItem::Agent(decl) => decl.symbol,
            HirItem::Protocol(decl) => decl.symbol,
            HirItem::Flow(decl) => decl.symbol,
            HirItem::Impl(_) | HirItem::Error { .. } => return None,
        };
        self.checked
            .hir
            .symbols
            .get(symbol)
            .map(|symbol| local_symbol_name(&symbol.name).to_owned())
    }

    fn item_path(&self, item: HirItemId) -> Option<Vec<String>> {
        let span = self.checked.source_map.item_sources.get(&item)?.span();
        for (_, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            for export in module.visibility_exports.items.values() {
                if export.item.span == span {
                    let mut path = module.path.segments.clone();
                    path.push(export.name.clone());
                    return Some(path);
                }
            }
        }
        None
    }

    fn action_path(&self, symbol: etas_hir::SymbolId) -> Option<Vec<String>> {
        let symbol = self.checked.hir.symbols.get(symbol)?;
        let etas_hir::SymbolDef::EffectAction { owner_effect, .. } = symbol.def else {
            return None;
        };
        let owner = owner_effect.and_then(|owner| self.checked.hir.symbols.get(owner))?;
        let module = self.checked.hir.modules_arena.get(symbol.defining_module)?;
        let mut path = module.name.as_ref().map(path_segments)?;
        path.push(local_symbol_name(&owner.name).to_owned());
        path.push(local_symbol_name(&symbol.name).to_owned());
        Some(path)
    }

    fn effect_ref_path(
        &self,
        effect_ref: &etas_hir::HirEffectRef,
    ) -> Result<Vec<String>, PackageMetadataError> {
        let unresolved = || PackageMetadataError::UnresolvedEffectPath {
            path: path_segments(&effect_ref.path),
        };
        match effect_ref.path.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => {
                if let Some(path) = self.symbol_canonical_path(symbol) {
                    Ok(path)
                } else {
                    Err(unresolved())
                }
            }
            etas_hir::ResolveResult::PartiallyResolved(_)
            | etas_hir::ResolveResult::Unresolved
            | etas_hir::ResolveResult::Ambiguous(_) => Err(unresolved()),
        }
    }

    fn symbol_canonical_path(&self, symbol: etas_hir::SymbolId) -> Option<Vec<String>> {
        let symbol = self.checked.hir.symbols.get(symbol)?;
        if let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def {
            return Some(path.clone());
        }
        let module = self.checked.hir.modules_arena.get(symbol.defining_module)?;
        let mut path = module.name.as_ref().map(path_segments)?;
        path.push(local_symbol_name(&symbol.name).to_owned());
        Some(path)
    }
}

fn metadata_action_arg_kind(kind: &etas_types::EffectActionArgKind) -> MetadataActionArgKind {
    match kind {
        etas_types::EffectActionArgKind::Type => MetadataActionArgKind::Type,
        etas_types::EffectActionArgKind::MemoryPlace => MetadataActionArgKind::MemoryPlace,
        etas_types::EffectActionArgKind::StaticResourcePath { ty } => {
            MetadataActionArgKind::StaticResourcePath { ty: ty.clone() }
        }
        etas_types::EffectActionArgKind::StringPattern => MetadataActionArgKind::StringPattern,
    }
}

fn build_canonical_type_paths(checked: &CheckedProject) -> BTreeMap<String, Option<Vec<String>>> {
    let mut paths = BTreeMap::<String, Option<Vec<String>>>::new();
    for (_, item) in checked.hir.items.iter() {
        let symbol = match item {
            HirItem::TypeAlias(decl) => decl.symbol,
            HirItem::Type(decl) => decl.symbol,
            HirItem::Enum(decl) => decl.symbol,
            _ => continue,
        };
        let Some(symbol_data) = checked.hir.symbols.get(symbol) else {
            continue;
        };
        let Some(path) = canonical_symbol_path(checked, symbol) else {
            continue;
        };
        let local = local_symbol_name(&symbol_data.name).to_owned();
        match paths.get_mut(&local) {
            Some(existing) if existing.as_ref() != Some(&path) => {
                *existing = None;
            }
            Some(_) => {}
            None => {
                paths.insert(local, Some(path));
            }
        }
    }
    paths
}

fn canonical_symbol_path(
    checked: &CheckedProject,
    symbol: etas_hir::SymbolId,
) -> Option<Vec<String>> {
    let symbol = checked.hir.symbols.get(symbol)?;
    if let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def {
        return Some(path.clone());
    }
    let local = local_symbol_name(&symbol.name).to_owned();
    let module = checked.hir.modules_arena.get(symbol.defining_module)?;
    let Some(module_name) = &module.name else {
        return Some(vec![local]);
    };
    let mut path = module_name
        .segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    path.push(local);
    Some(path)
}

fn path_segments(path: &etas_hir::ResolvedPath) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect()
}

fn local_symbol_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn json_schema_for_type(
    ty: TypeId,
    store: &TypeStore,
    depth: usize,
) -> Result<Value, PackageMetadataError> {
    if depth > 16 {
        return Err(PackageMetadataError::UnsupportedType {
            ty,
            reason: "tool schema type nesting exceeds metadata limit".to_owned(),
        });
    }
    let Some(data) = store.get(ty).cloned() else {
        return Err(PackageMetadataError::UnsupportedType {
            ty,
            reason: "missing checked type facts".to_owned(),
        });
    };
    match data {
        Type::Primitive(primitive) => Ok(json_schema_for_primitive(primitive)),
        Type::IntegerLiteral { .. } => Ok(json!({ "type": "integer" })),
        Type::Array(inner) | Type::List(inner) | Type::Set(inner) | Type::Slice(inner) => {
            Ok(json!({
                "type": "array",
                "items": json_schema_for_type(inner, store, depth + 1)?,
            }))
        }
        Type::Map { key, value } => {
            if matches!(store.get(key), Some(Type::Primitive(PrimitiveType::String))) {
                Ok(json!({
                    "type": "object",
                    "additionalProperties": json_schema_for_type(value, store, depth + 1)?,
                }))
            } else {
                Ok(json!({
                    "type": "array",
                    "items": {
                        "type": "array",
                        "prefixItems": [
                            json_schema_for_type(key, store, depth + 1)?,
                            json_schema_for_type(value, store, depth + 1)?,
                        ],
                        "minItems": 2,
                        "maxItems": 2,
                    },
                }))
            }
        }
        Type::Option(inner) => Ok(json!({
            "anyOf": [
                json_schema_for_type(inner, store, depth + 1)?,
                { "type": "null" },
            ],
        })),
        Type::Result { ok, err } => Ok(json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "name": { "const": "Ok" },
                        "fields": {
                            "type": "array",
                            "prefixItems": [json_schema_for_type(ok, store, depth + 1)?],
                            "minItems": 1,
                            "maxItems": 1,
                        },
                    },
                    "required": ["name", "fields"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "properties": {
                        "name": { "const": "Err" },
                        "fields": {
                            "type": "array",
                            "prefixItems": [json_schema_for_type(err, store, depth + 1)?],
                            "minItems": 1,
                            "maxItems": 1,
                        },
                    },
                    "required": ["name", "fields"],
                    "additionalProperties": false,
                },
            ],
        })),
        Type::Record(record) => {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::with_capacity(record.fields.len());
            for field in record.fields {
                properties.insert(
                    field.name.clone(),
                    json_schema_for_type(field.ty, store, depth + 1)?,
                );
                required.push(Value::String(field.name));
            }
            Ok(json!({
                "type": "object",
                "properties": Value::Object(properties),
                "required": required,
                "additionalProperties": false,
            }))
        }
        Type::Tuple(elements) => {
            let items = elements
                .into_iter()
                .map(|element| json_schema_for_type(element, store, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({
                "type": "array",
                "prefixItems": items,
                "minItems": items.len(),
                "maxItems": items.len(),
            }))
        }
        Type::Trust { inner, .. }
        | Type::Schema(inner)
        | Type::Message(inner)
        | Type::MemorySelection(inner)
        | Type::MemoryRegion(inner)
        | Type::Refined { base: inner, .. } => json_schema_for_type(inner, store, depth + 1),
        Type::Nominal(nominal) => {
            let Some(representation) = nominal.representation else {
                return Err(PackageMetadataError::UnsupportedType {
                    ty,
                    reason: format!(
                        "`{}` has no representation and cannot be lowered to a model tool JSON schema",
                        nominal.name
                    ),
                });
            };
            json_schema_for_type(representation, store, depth + 1)
        }
        Type::Prompt | Type::PromptPart => Ok(json!({ "type": "string" })),
        Type::Named(named) if named.name == "Json" => Ok(json!({})),
        other => Err(PackageMetadataError::UnsupportedType {
            ty,
            reason: format!("`{other:?}` cannot be lowered to a model tool JSON schema"),
        }),
    }
}

fn json_schema_for_primitive(primitive: PrimitiveType) -> Value {
    match primitive {
        PrimitiveType::Bool => json!({ "type": "boolean" }),
        PrimitiveType::I8
        | PrimitiveType::I16
        | PrimitiveType::I32
        | PrimitiveType::I64
        | PrimitiveType::I128
        | PrimitiveType::ISize => json!({ "type": "integer" }),
        PrimitiveType::U8
        | PrimitiveType::U16
        | PrimitiveType::U32
        | PrimitiveType::U64
        | PrimitiveType::U128
        | PrimitiveType::USize => json!({ "type": "integer", "minimum": 0 }),
        PrimitiveType::F32 | PrimitiveType::F64 => json!({ "type": "number" }),
        PrimitiveType::String | PrimitiveType::Char => json!({ "type": "string" }),
        PrimitiveType::Bytes => json!({
            "type": "array",
            "items": { "type": "integer", "minimum": 0, "maximum": 255 },
        }),
        PrimitiveType::Unit | PrimitiveType::Never => json!({ "type": "null" }),
    }
}

fn metadata_type(ty: TypeId, store: &TypeStore) -> Result<MetadataType, PackageMetadataError> {
    let Some(ty_value) = store.get(ty) else {
        return Err(PackageMetadataError::UnsupportedType {
            ty,
            reason: "type id is missing from type store".to_owned(),
        });
    };
    let mut metadata = MetadataType::default();
    match ty_value {
        Type::Primitive(primitive) => {
            metadata.kind = MetadataTypeKind::Primitive;
            metadata.name = primitive_name(*primitive).to_owned();
        }
        Type::IntegerLiteral { text } => {
            metadata.kind = MetadataTypeKind::Primitive;
            metadata.name = text.clone();
        }
        Type::Var(var) => {
            metadata.kind = MetadataTypeKind::Var;
            metadata.name = format!("T{}", var.0);
        }
        Type::Array(element) => {
            unary_type(MetadataTypeKind::Array, *element, store, &mut metadata)?
        }
        Type::List(element) => unary_type(MetadataTypeKind::List, *element, store, &mut metadata)?,
        Type::Map { key, value } => {
            metadata.kind = MetadataTypeKind::Map;
            metadata.children.push(metadata_type(*key, store)?);
            metadata.children.push(metadata_type(*value, store)?);
        }
        Type::Set(element) => unary_type(MetadataTypeKind::Set, *element, store, &mut metadata)?,
        Type::Range { index } => unary_type(MetadataTypeKind::Range, *index, store, &mut metadata)?,
        Type::Slice(element) => {
            unary_type(MetadataTypeKind::Slice, *element, store, &mut metadata)?
        }
        Type::Option(inner) => unary_type(MetadataTypeKind::Option, *inner, store, &mut metadata)?,
        Type::Result { ok, err } => {
            metadata.kind = MetadataTypeKind::Result;
            metadata.children.push(metadata_type(*ok, store)?);
            metadata.children.push(metadata_type(*err, store)?);
        }
        Type::Tuple(elements) => {
            metadata.kind = MetadataTypeKind::Tuple;
            metadata.children = elements
                .iter()
                .map(|element| metadata_type(*element, store))
                .collect::<Result<Vec<_>, _>>()?;
        }
        Type::Function(flow) => {
            metadata.kind = MetadataTypeKind::Function;
            metadata.children = flow
                .input
                .iter()
                .map(|input| metadata_type(*input, store))
                .collect::<Result<Vec<_>, _>>()?;
            metadata.children.push(metadata_type(flow.output, store)?);
            metadata.effects = flow
                .effects
                .as_ref()
                .map(|row| metadata_effect_row_ref(row, store))
                .transpose()?;
        }
        Type::Handler(handler) => {
            metadata.kind = MetadataTypeKind::Handler;
            metadata.effects = Some(metadata_effect_row_ref(&handler.handled, store)?);
            metadata.produced_effects = match &handler.produced {
                etas_types::HandlerProducedEffects::Infer => None,
                etas_types::HandlerProducedEffects::Explicit(row) => {
                    Some(metadata_effect_row_ref(row, store)?)
                }
            };
            if let Some(result) = handler.result {
                metadata.children.push(metadata_type(result, store)?);
            }
        }
        Type::Enum(enum_ref) => {
            metadata.kind = MetadataTypeKind::Named;
            metadata.path = enum_ref.name.split('.').map(str::to_owned).collect();
        }
        Type::Named(named) => {
            metadata.kind = MetadataTypeKind::Named;
            metadata.path = named.name.split('.').map(str::to_owned).collect();
        }
        Type::Nominal(nominal) => {
            metadata.kind = MetadataTypeKind::Nominal;
            metadata.path = nominal.name.split('.').map(str::to_owned).collect();
            if let Some(representation) = nominal.representation {
                metadata
                    .children
                    .push(metadata_type(representation, store)?);
            }
        }
        Type::Applied { constructor, args } => {
            metadata.kind = MetadataTypeKind::Applied;
            metadata.path = applied_type_constructor_path(*constructor, store)?;
            metadata.children = args
                .iter()
                .map(|arg| metadata_type(*arg, store))
                .collect::<Result<Vec<_>, _>>()?;
        }
        Type::Trust { wrapper, inner } => {
            metadata.kind = trust_wrapper_kind(*wrapper);
            metadata.children.push(metadata_type(*inner, store)?);
        }
        Type::Prompt => metadata.kind = MetadataTypeKind::Prompt,
        Type::PromptPart => metadata.kind = MetadataTypeKind::PromptPart,
        Type::Message(inner) => {
            unary_type(MetadataTypeKind::Message, *inner, store, &mut metadata)?
        }
        Type::MemorySelection(inner) => unary_type(
            MetadataTypeKind::MemorySelection,
            *inner,
            store,
            &mut metadata,
        )?,
        Type::Store { key, value } => {
            metadata.kind = MetadataTypeKind::Store;
            metadata.children.push(metadata_type(*key, store)?);
            metadata.children.push(metadata_type(*value, store)?);
        }
        Type::MemoryRegion(schema) => unary_type(
            MetadataTypeKind::MemoryRegion,
            *schema,
            store,
            &mut metadata,
        )?,
        Type::ResourceHandle(handle) => {
            metadata.kind = MetadataTypeKind::ResourceHandle;
            match handle {
                ResourceHandleType::MemoryRegion { schema } => {
                    metadata.name = "MemoryRegion".to_owned();
                    metadata.children.push(metadata_type(*schema, store)?);
                }
                ResourceHandleType::ExternalTool { signature } => {
                    metadata.name = "ExternalTool".to_owned();
                    metadata.children.push(metadata_type(*signature, store)?);
                }
                ResourceHandleType::Other { name, args } => {
                    metadata.name = name.clone();
                    metadata.children = args
                        .iter()
                        .map(|arg| metadata_type(*arg, store))
                        .collect::<Result<Vec<_>, _>>()?;
                }
            }
        }
        Type::Record(record) => {
            metadata.kind = MetadataTypeKind::Record;
            metadata.fields = record
                .fields
                .iter()
                .map(|field| {
                    Ok(MetadataTypeField {
                        name: field.name.clone(),
                        ty: metadata_type(field.ty, store)?,
                    })
                })
                .collect::<Result<Vec<_>, PackageMetadataError>>()?;
        }
        Type::Refined { .. } | Type::Schema(_) | Type::MemoryPlace(_) => {
            return Err(PackageMetadataError::UnsupportedType {
                ty,
                reason: format!("{ty_value:?} has no published metadata schema yet"),
            });
        }
    }
    Ok(metadata)
}

fn applied_type_constructor_path(
    constructor: etas_types::TypeConstructorId,
    store: &TypeStore,
) -> Result<Vec<String>, PackageMetadataError> {
    let ty = TypeId(constructor.0);
    let Some(ty_value) = store.get(ty) else {
        return Err(PackageMetadataError::UnsupportedType {
            ty,
            reason: "applied type constructor is missing from type store".to_owned(),
        });
    };
    let path = match ty_value {
        Type::Named(named) => &named.name,
        Type::Nominal(nominal) => &nominal.name,
        Type::Enum(enum_ref) => &enum_ref.name,
        other => {
            return Err(PackageMetadataError::UnsupportedType {
                ty,
                reason: format!("applied type constructor {other:?} has no metadata path"),
            });
        }
    };
    Ok(path.split('.').map(str::to_owned).collect())
}

fn metadata_alias_type(
    path: &[String],
    target: TypeId,
    store: &TypeStore,
) -> Result<MetadataType, PackageMetadataError> {
    Ok(MetadataType {
        kind: MetadataTypeKind::Alias,
        path: path.to_vec(),
        children: vec![metadata_type(target, store)?],
        ..Default::default()
    })
}

fn canonicalize_declared_type_path(metadata: &mut MetadataType, path: &[String]) {
    match metadata.kind {
        MetadataTypeKind::Named | MetadataTypeKind::Nominal => {
            metadata.path = path.to_vec();
        }
        _ => {}
    }
}

fn unary_type(
    kind: MetadataTypeKind,
    inner: TypeId,
    store: &TypeStore,
    metadata: &mut MetadataType,
) -> Result<(), PackageMetadataError> {
    metadata.kind = kind;
    metadata.children.push(metadata_type(inner, store)?);
    Ok(())
}

fn metadata_effect_row_ref(
    row: &EffectRowRef,
    store: &TypeStore,
) -> Result<MetadataEffectRow, PackageMetadataError> {
    Ok(MetadataEffectRow {
        effects: row
            .effects
            .iter()
            .map(|effect| metadata_effect_ref(effect, store))
            .collect::<Result<Vec<_>, _>>()?,
        tail: row.tail.clone(),
    })
}

fn metadata_effect_ref(
    effect: &EffectRef,
    store: &TypeStore,
) -> Result<MetadataEffectRef, PackageMetadataError> {
    Ok(MetadataEffectRef {
        path: effect.name.split('.').map(str::to_owned).collect(),
        args: effect
            .args
            .iter()
            .map(|arg| metadata_effect_arg(arg, store))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn metadata_effect_arg(
    arg: &EffectArgRef,
    store: &TypeStore,
) -> Result<MetadataEffectArg, PackageMetadataError> {
    match arg {
        EffectArgRef::Type(ty) => Ok(MetadataEffectArg {
            kind: MetadataEffectArgKind::Type,
            ty: Some(metadata_type(*ty, store)?),
            path: Vec::new(),
            value: String::new(),
        }),
        EffectArgRef::Wildcard => Ok(MetadataEffectArg {
            kind: MetadataEffectArgKind::Wildcard,
            ty: None,
            path: Vec::new(),
            value: String::new(),
        }),
        EffectArgRef::String(value) => Ok(MetadataEffectArg {
            kind: MetadataEffectArgKind::String,
            ty: None,
            path: Vec::new(),
            value: value.clone(),
        }),
        EffectArgRef::Int(value) => Ok(MetadataEffectArg {
            kind: MetadataEffectArgKind::Int,
            ty: None,
            path: Vec::new(),
            value: value.clone(),
        }),
        EffectArgRef::Path(path) => Ok(MetadataEffectArg {
            kind: MetadataEffectArgKind::Path,
            ty: None,
            path: path.clone(),
            value: String::new(),
        }),
    }
}

fn callable_type_generic_names(signature: &ItemSignature) -> BTreeSet<String> {
    match signature {
        ItemSignature::Flow(signature)
        | ItemSignature::Agent(signature)
        | ItemSignature::Tool(signature) => signature
            .generic_params
            .iter()
            .filter(|param| param.kind == etas_types::CallableGenericParamKind::Type)
            .map(|param| param.name.clone())
            .collect(),
        ItemSignature::TopLevelLet(_) => BTreeSet::new(),
    }
}

fn callable_effect_generic_names(signature: &ItemSignature) -> BTreeSet<String> {
    match signature {
        ItemSignature::Flow(signature)
        | ItemSignature::Agent(signature)
        | ItemSignature::Tool(signature) => signature
            .generic_params
            .iter()
            .filter(|param| param.kind == etas_types::CallableGenericParamKind::Effect)
            .map(|param| param.name.clone())
            .collect(),
        ItemSignature::TopLevelLet(_) => BTreeSet::new(),
    }
}

fn normalize_generic_effect_row(row: &mut MetadataEffectRow, names: &BTreeSet<String>) {
    for effect in &mut row.effects {
        for arg in &mut effect.args {
            if let Some(ty) = &mut arg.ty {
                normalize_generic_type(ty, names);
            }
        }
    }
}

fn normalize_generic_type(ty: &mut MetadataType, names: &BTreeSet<String>) {
    if matches!(ty.kind, MetadataTypeKind::Named)
        && let [name] = ty.path.as_slice()
        && names.contains(name)
    {
        ty.kind = MetadataTypeKind::Var;
        ty.name = name.clone();
        ty.path.clear();
    }
    for child in &mut ty.children {
        normalize_generic_type(child, names);
    }
    for field in &mut ty.fields {
        normalize_generic_type(&mut field.ty, names);
    }
    if let Some(effects) = &mut ty.effects {
        normalize_generic_effect_row(effects, names);
    }
    if let Some(effects) = &mut ty.produced_effects {
        normalize_generic_effect_row(effects, names);
    }
}

fn public_symbols(metadata: &PublicMetadata) -> PublicSymbols {
    let mut symbols = Vec::new();
    for symbol in &metadata.types {
        symbols.push(public_symbol("type", symbol));
    }
    for symbol in &metadata.values {
        symbols.push(public_symbol("value", symbol));
    }
    for symbol in &metadata.enums {
        symbols.push(public_symbol("enum", symbol));
    }
    for symbol in &metadata.flows {
        symbols.push(public_callable_symbol("flow", symbol));
    }
    for symbol in &metadata.agents {
        symbols.push(public_callable_symbol("agent", symbol));
    }
    for symbol in &metadata.tools {
        symbols.push(public_callable_symbol("tool", symbol));
    }
    for symbol in &metadata.effects {
        symbols.push(public_symbol("effect", symbol));
    }
    for symbol in &metadata.actions {
        symbols.push(PublicSymbol {
            kind: "action".to_owned(),
            path: symbol.path.clone(),
            visibility: symbol.visibility,
        });
    }
    for symbol in &metadata.trace_specs {
        symbols.push(public_symbol("trace_spec", symbol));
    }
    for symbol in &metadata.protocols {
        symbols.push(public_symbol("protocol", symbol));
    }
    PublicSymbols { symbols }
}

fn public_symbol(kind: &str, symbol: &MetadataNamedSignature) -> PublicSymbol {
    PublicSymbol {
        kind: kind.to_owned(),
        path: symbol.path.clone(),
        visibility: symbol.visibility,
    }
}

fn named_signature(path: Vec<String>, visibility: MetadataVisibility) -> MetadataNamedSignature {
    MetadataNamedSignature {
        path,
        visibility,
        ty: None,
    }
}

fn public_callable_symbol(kind: &str, symbol: &MetadataCallableSignature) -> PublicSymbol {
    PublicSymbol {
        kind: kind.to_owned(),
        path: symbol.path.clone(),
        visibility: symbol.visibility,
    }
}

fn visibility_name(visibility: Visibility) -> MetadataVisibility {
    match visibility {
        Visibility::Public => MetadataVisibility::Public,
        Visibility::Private => MetadataVisibility::Private,
    }
}

fn primitive_name(primitive: PrimitiveType) -> &'static str {
    primitive.source_name()
}

fn trust_wrapper_kind(wrapper: TrustWrapper) -> MetadataTypeKind {
    match wrapper {
        TrustWrapper::Trusted => MetadataTypeKind::Trusted,
        TrustWrapper::Untrusted => MetadataTypeKind::Untrusted,
        TrustWrapper::Secret => MetadataTypeKind::Secret,
        TrustWrapper::Public => MetadataTypeKind::Public,
        TrustWrapper::Sanitized => MetadataTypeKind::Sanitized,
    }
}
