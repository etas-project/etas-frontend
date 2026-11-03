use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint,
    descriptor::{DescriptorLimits, GenericOwner, GenericParameterKind},
    residual::identity::{
        GenericArgumentIdentity, GenericParameterId, ResidualOwnerId, ResidualOwnerKind,
        SourceRevisionLocation,
    },
};
use etas_types::{CallableGenericArg, EffectRowRef};

use super::DefinitionCatalog;
use crate::{
    CheckedCallableApplication,
    code::descriptor::{DefinitionDescriptorProjector, ProjectionError},
};

impl DefinitionCatalog {
    pub fn is_external_callable_reflection(
        &self,
        expression: etas_hir::HirExprId,
    ) -> Result<bool, ProjectionError> {
        let super::StaticReflectionView::Callable(header) = self.static_reflection(expression)?
        else {
            return Err(ProjectionError::Invalid(
                "reflection is not a callable".into(),
            ));
        };
        Ok(self.external_callables.contains_key(&header.id))
    }

    pub(super) fn residual_source_owner(
        &self,
        item: etas_hir::HirItemId,
    ) -> Result<ResidualOwnerId, ProjectionError> {
        let declaration = self.checked.hir.items.get(item).ok_or_else(|| {
            ProjectionError::MissingIdentity("residual source declaration".into())
        })?;
        if !matches!(declaration, etas_hir::HirItem::Flow(_)) {
            return Err(ProjectionError::Invalid(
                "residual application requires a source flow".into(),
            ));
        }
        let span = declaration.span();
        let mut sources = self
            .checked
            .sources
            .sources
            .iter()
            .filter(|source| source.id == span.source);
        let source = sources
            .next()
            .ok_or_else(|| ProjectionError::MissingIdentity("residual source snapshot".into()))?;
        if sources.next().is_some() {
            return Err(ProjectionError::Invalid(
                "ambiguous residual source snapshot".into(),
            ));
        }
        let key = self
            .checked
            .sources
            .stable_source_key(span.source)
            .map_err(|reason| {
                ProjectionError::MissingIdentity(format!(
                    "stable source owner for residual application: {reason:?}"
                ))
            })?;
        let location = SourceRevisionLocation::new(
            key,
            source.text.as_bytes(),
            u64::from(span.range.start.0),
            u64::from(span.range.end.0),
        )
        .map_err(|reason| {
            ProjectionError::Invalid(format!(
                "invalid residual declaration source location: {reason:?}"
            ))
        })?;
        ResidualOwnerId::from_source_declaration(
            &location,
            source.text.as_bytes(),
            ResidualOwnerKind::Flow,
        )
        .map_err(|reason| {
            ProjectionError::Invalid(format!("invalid residual source owner: {reason:?}"))
        })
    }

    /// Portable binder-position identity for a sealed native application. No
    /// TypeId, HIR ID, or descriptive type name participates in the digest.
    pub fn callable_application_fingerprint(
        &self,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<CodeFingerprint, ProjectionError> {
        let (callable, _, arguments) = self.portable_application_arguments(application, limits)?;
        let application_id = application_fingerprint(&callable, arguments)?;
        let Some(interface) = application.presented_type() else {
            return Ok(application_id);
        };
        let owner = callable
            .as_declaration()
            .cloned()
            .map(GenericOwner::Declaration)
            .or_else(|| callable.as_method().cloned().map(GenericOwner::Method))
            .ok_or_else(|| ProjectionError::Invalid("callable has no generic owner".into()))?;
        let mut projector =
            DefinitionDescriptorProjector::new(&self.checked, &self.identities, limits);
        let projected = projector.project_closed_types(owner, &[interface])?;
        let graph = projector.finish()?;
        let interface_ref = projected.first().copied().ok_or_else(|| {
            ProjectionError::MissingIdentity("presented callable interface".into())
        })?;
        let interface_id = graph.type_fingerprint(interface_ref)?;
        Ok(CodeFingerprint::of_parts(
            "etas.code.presented-callable-application.v1",
            [
                application_id.as_bytes().as_slice(),
                interface_id.as_bytes().as_slice(),
            ],
        ))
    }

    pub fn external_callable_application_fingerprint(
        &self,
        application: &super::CheckedExternalCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<CodeFingerprint, ProjectionError> {
        if !Arc::ptr_eq(application.program(), &self.checked)
            || !self.external_callables.contains_key(application.callable())
        {
            return Err(ProjectionError::Invalid(
                "external application belongs to a different checked catalog".into(),
            ));
        }
        let (callable, _, arguments) =
            self.portable_arguments(application.callable(), application.arguments(), limits)?;
        application_fingerprint(&callable, arguments)
    }

    /// Projects only the applied callable signature. This is descriptive
    /// data; requested actions and traces require independent checked
    /// behavior specialization before they can form a frozen contract.
    pub fn external_callable_application_signature(
        &self,
        application: &super::CheckedExternalCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<
        (
            etas_code::descriptor::DescriptorGraph,
            etas_code::descriptor::TypeInfoRef,
        ),
        ProjectionError,
    > {
        let (graph, header) = self.project_external_application(application, limits, false)?;
        Ok((graph, header.signature))
    }

    /// Specializes the producer's descriptive signature and behavior graph.
    /// Callback placeholders remain placeholders; this is not an execution
    /// permit or a substitute for solving their runtime call sites.
    pub fn external_callable_application_description(
        &self,
        application: &super::CheckedExternalCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<
        (
            etas_code::descriptor::DescriptorGraph,
            etas_code::descriptor::CallableHeaderInfo,
        ),
        ProjectionError,
    > {
        self.project_external_application(application, limits, true)
    }

    fn project_external_application(
        &self,
        application: &super::CheckedExternalCallableApplication,
        limits: DescriptorLimits,
        include_behavior: bool,
    ) -> Result<
        (
            etas_code::descriptor::DescriptorGraph,
            etas_code::descriptor::CallableHeaderInfo,
        ),
        ProjectionError,
    > {
        use etas_code::descriptor::{
            DescriptorArgument, DescriptorGraph, DescriptorGraphOffsets, DescriptorNode, TypeInfo,
        };
        if !Arc::ptr_eq(application.program(), &self.checked) {
            return Err(ProjectionError::Invalid(
                "external application belongs to a different checked catalog".into(),
            ));
        }
        let external = self
            .external_callables
            .get(application.callable())
            .ok_or_else(|| {
                ProjectionError::Invalid("external application has no validated metadata".into())
            })?;
        external.graph.validate(limits)?;
        let declaration = application.callable().as_declaration().ok_or_else(|| {
            ProjectionError::Invalid("external flow has no declaration identity".into())
        })?;
        let owner = GenericOwner::Declaration(declaration.clone());
        let mut header = external.header.clone();
        let mut source_roots = vec![DescriptorNode::Type(header.signature)];
        if include_behavior {
            let effects = &header.analysis.effects;
            source_roots.extend(
                [
                    effects.declared_bound,
                    effects.escaping,
                    effects.requested,
                    effects.default_handled,
                ]
                .into_iter()
                .flatten()
                .map(DescriptorNode::Row),
            );
            source_roots.extend(
                [header.analysis.trace, header.analysis.outcomes]
                    .into_iter()
                    .flatten()
                    .map(DescriptorNode::Trace),
            );
        }
        let (signature_graph, roots) = external.graph.project_roots(&source_roots)?;
        let mut roots = roots.into_iter();
        let Some(DescriptorNode::Type(signature)) = roots.next() else {
            return Err(ProjectionError::Invalid(
                "external flow signature projection is invalid".into(),
            ));
        };
        header.signature = signature;
        if include_behavior {
            let effects = &mut header.analysis.effects;
            for row in [
                &mut effects.declared_bound,
                &mut effects.escaping,
                &mut effects.requested,
                &mut effects.default_handled,
            ] {
                if row.is_some() {
                    let Some(DescriptorNode::Row(mapped)) = roots.next() else {
                        return Err(ProjectionError::Invalid(
                            "external behavior row projection is invalid".into(),
                        ));
                    };
                    *row = Some(mapped);
                }
            }
            for trace in [&mut header.analysis.trace, &mut header.analysis.outcomes] {
                if trace.is_some() {
                    let Some(DescriptorNode::Trace(mapped)) = roots.next() else {
                        return Err(ProjectionError::Invalid(
                            "external behavior trace projection is invalid".into(),
                        ));
                    };
                    *trace = Some(mapped);
                }
            }
        }
        if roots.next().is_some() {
            return Err(ProjectionError::Invalid(
                "external projection contains unexpected roots".into(),
            ));
        }
        let Some(TypeInfo::Function(function)) = signature_graph.type_info(signature) else {
            return Err(ProjectionError::Invalid(
                "external flow has no callable signature".into(),
            ));
        };
        if function.generics.len() != application.arguments().len() {
            return Err(ProjectionError::Invalid(
                "external application binder arity differs from metadata".into(),
            ));
        }
        let binders = function.generics.clone();
        let mut projection =
            DefinitionDescriptorProjector::new(&self.checked, &self.identities, limits);
        let mut roots = Vec::with_capacity(application.arguments().len());
        for argument in application.arguments() {
            roots.push(match argument {
                CallableGenericArg::Type(ty) => DescriptorNode::Type(
                    *projection
                        .project_closed_types(owner.clone(), &[*ty])?
                        .first()
                        .ok_or(ProjectionError::MissingType(*ty))?,
                ),
                CallableGenericArg::EffectRow(row) => DescriptorNode::Row(
                    *projection
                        .project_closed_rows(owner.clone(), std::slice::from_ref(row))?
                        .first()
                        .ok_or_else(|| {
                            ProjectionError::Invalid("external application row is missing".into())
                        })?,
                ),
            });
        }
        let arguments = projection.finish()?;
        let original = signature_graph.data();
        let offset = |size: usize| {
            u32::try_from(size).map_err(|_| ProjectionError::Limit {
                resource: "descriptor arena indices",
                limit: u32::MAX as usize,
            })
        };
        let offsets = DescriptorGraphOffsets {
            types: offset(original.types.len())?,
            rows: offset(original.rows.len())?,
            binders: offset(original.binders.len())?,
            traces: offset(original.traces.len())?,
        };
        let (segment, roots) = arguments.relocate_roots(&roots, offsets)?;
        let nodes = original
            .types
            .len()
            .checked_add(original.rows.len())
            .and_then(|n| n.checked_add(original.binders.len()))
            .and_then(|n| n.checked_add(original.traces.len()))
            .and_then(|n| n.checked_add(segment.types.len()))
            .and_then(|n| n.checked_add(segment.rows.len()))
            .and_then(|n| n.checked_add(segment.binders.len()))
            .and_then(|n| n.checked_add(segment.traces.len()))
            .ok_or(ProjectionError::Limit {
                resource: "nodes",
                limit: limits.max_nodes,
            })?;
        if nodes > limits.max_nodes {
            return Err(ProjectionError::Limit {
                resource: "nodes",
                limit: limits.max_nodes,
            });
        }
        let mut merged = original.clone();
        merged.types.extend(segment.types);
        merged.rows.extend(segment.rows);
        merged.binders.extend(segment.binders);
        merged.traces.extend(segment.traces);
        let graph = DescriptorGraph::new_with_merged_binders(merged, limits)?;
        let bindings = binders
            .into_iter()
            .zip(roots)
            .map(|(binder, node)| {
                let arg = match node {
                    DescriptorNode::Type(ty) => DescriptorArgument::Type(ty),
                    DescriptorNode::Row(row) => DescriptorArgument::EffectRow(row),
                    _ => {
                        return Err(ProjectionError::Invalid(
                            "external application has an invalid descriptor argument".into(),
                        ));
                    }
                };
                Ok((binder, arg))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((
            graph.specialize_generics(signature, &bindings, limits)?,
            header,
        ))
    }

    pub(crate) fn residual_application_bindings(
        &self,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
        source_owner: ResidualOwnerId,
    ) -> Result<BTreeMap<GenericParameterId, GenericArgumentIdentity>, ProjectionError> {
        let (_, _, arguments) = self.portable_application_arguments(application, limits)?;
        let signature =
            crate::project::instantiation::signature(&self.checked.types, application.owner())
                .ok_or_else(|| {
                    ProjectionError::Invalid("checked application signature missing".into())
                })?;
        if signature.generic_params.len() != arguments.len() {
            return Err(ProjectionError::Invalid(
                "application binder arity differs".into(),
            ));
        }
        let mut bindings = BTreeMap::new();
        for (position, (parameter, argument)) in
            signature.generic_params.iter().zip(arguments).enumerate()
        {
            let kind = match (parameter.kind, argument) {
                (etas_types::CallableGenericParamKind::Type, GenericArgumentIdentity::Type(_)) => {
                    GenericParameterKind::Type
                }
                (
                    etas_types::CallableGenericParamKind::Effect,
                    GenericArgumentIdentity::EffectRow(_),
                ) => GenericParameterKind::EffectRow,
                _ => {
                    return Err(ProjectionError::Invalid(
                        "application binder kind differs".into(),
                    ));
                }
            };
            let position = u32::try_from(position).map_err(|_| {
                ProjectionError::Invalid("application binder position exceeds limit".into())
            })?;
            let id = GenericParameterId::from_source_owner(source_owner, position, kind);
            if bindings.insert(id, argument).is_some() {
                return Err(ProjectionError::Invalid(
                    "duplicate application binder identity".into(),
                ));
            }
        }
        Ok(bindings)
    }

    fn portable_application_arguments(
        &self,
        application: &CheckedCallableApplication,
        limits: DescriptorLimits,
    ) -> Result<
        (
            etas_code::CallableId,
            GenericOwner,
            Vec<GenericArgumentIdentity>,
        ),
        ProjectionError,
    > {
        if !Arc::ptr_eq(application.program(), &self.checked) {
            return Err(ProjectionError::Invalid(
                "callable application belongs to a different checked program".into(),
            ));
        }
        let symbol = self
            .checked
            .items
            .get(application.owner())
            .and_then(etas_hir::HirItem::symbol)
            .ok_or_else(|| {
                ProjectionError::MissingIdentity("callable application declaration".into())
            })?;
        let callable = self
            .callable_bindings
            .get(&symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        self.portable_arguments(callable, application.arguments(), limits)
    }

    fn portable_arguments(
        &self,
        callable: &etas_code::CallableId,
        arguments: &[CallableGenericArg],
        limits: DescriptorLimits,
    ) -> Result<
        (
            etas_code::CallableId,
            GenericOwner,
            Vec<GenericArgumentIdentity>,
        ),
        ProjectionError,
    > {
        let owner = match (callable.as_declaration(), callable.as_method()) {
            (Some(declaration), None) => GenericOwner::Declaration(declaration.clone()),
            (None, Some(method)) => GenericOwner::Method(method.clone()),
            _ => {
                return Err(ProjectionError::Invalid(
                    "callable application has no canonical owner".into(),
                ));
            }
        };
        let mut projection =
            DefinitionDescriptorProjector::new(&self.checked, &self.identities, limits);
        let mut types = Vec::new();
        let mut rows = Vec::new();
        let mut positions = Vec::new();
        for argument in arguments {
            match argument {
                CallableGenericArg::Type(ty) => {
                    let projected = projection.project_closed_types(owner.clone(), &[*ty])?;
                    types.push(*projected.first().ok_or(ProjectionError::MissingType(*ty))?);
                    positions.push(Argument::Type(types.len() - 1));
                }
                CallableGenericArg::EffectRow(row) => {
                    let members = row
                        .effects
                        .iter()
                        .cloned()
                        .map(|effect| EffectRowRef {
                            effects: vec![effect],
                            tail: None,
                        })
                        .collect::<Vec<_>>();
                    rows.push(projection.project_closed_rows(owner.clone(), &members)?);
                    positions.push(Argument::Row(rows.len() - 1));
                }
            }
        }
        let graph = projection.finish()?;
        let mut values = Vec::with_capacity(positions.len());
        for argument in positions {
            values.push(match argument {
                Argument::Type(index) => {
                    GenericArgumentIdentity::Type(graph.type_fingerprint(types[index])?)
                }
                Argument::Row(index) => {
                    let mut members = rows[index]
                        .iter()
                        .map(|row| graph.row_fingerprint(*row))
                        .collect::<Result<Vec<_>, _>>()?;
                    members.sort_unstable();
                    members.dedup();
                    GenericArgumentIdentity::EffectRow(CodeFingerprint::of_parts(
                        "etas.code.application-row.v1",
                        members.iter().map(|member| member.as_bytes().as_slice()),
                    ))
                }
            });
        }
        Ok((callable.clone(), owner, values))
    }
}

fn application_fingerprint(
    callable: &etas_code::CallableId,
    arguments: Vec<GenericArgumentIdentity>,
) -> Result<CodeFingerprint, ProjectionError> {
    let mut fingerprints = vec![callable.fingerprint()];
    for (position, argument) in arguments.into_iter().enumerate() {
        let (domain, value) = match argument {
            GenericArgumentIdentity::Type(value) => {
                ("etas.code.application-type-argument.v1", value)
            }
            GenericArgumentIdentity::EffectRow(value) => {
                ("etas.code.application-effect-argument.v1", value)
            }
            _ => {
                return Err(ProjectionError::Invalid(
                    "application argument is not closed".into(),
                ));
            }
        };
        fingerprints.push(CodeFingerprint::of_parts(
            domain,
            [
                (position as u64).to_le_bytes().as_slice(),
                value.as_bytes().as_slice(),
            ],
        ));
    }
    Ok(CodeFingerprint::of_parts(
        "etas.code.callable-application.v1",
        fingerprints
            .iter()
            .map(|fingerprint| fingerprint.as_bytes().as_slice()),
    ))
}

#[derive(Clone, Copy)]
enum Argument {
    Type(usize),
    Row(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EntryPolicy, Frontend, ProjectInput, ProjectStableSourceKeyInput, SourceInput,
        code::identity::DefinitionOrigins,
    };
    use etas_code::{
        PackageKey, StableSourceKey, StableSourceOrigin,
        residual::identity::{ObligationId, ResidualObligationRef},
    };
    use etas_types::{EffectRowRef, PrimitiveType, Type};

    fn source_key() -> StableSourceKey {
        StableSourceKey::new(
            StableSourceOrigin::Project(CodeFingerprint::from_bytes([71; 32])),
            &["main.es".into()],
        )
        .unwrap()
    }

    fn checked() -> crate::CheckedProgram {
        checked_with_source(etas_core::SourceId(0), true)
    }

    fn checked_with_source(id: etas_core::SourceId, with_key: bool) -> crate::CheckedProgram {
        let mut source = SourceInput::anonymous(
            "module app; flow both<T, effect E>(value: T, callback: () -> unit ![E]) -> T ![E] { callback(); return value; }
             flow noop() -> unit ![] { return; }
             flow prepare() -> bool ![] { return both<bool, ![]>(true, noop); }",
        );
        source.id = id;
        let mut input = ProjectInput::single_source(source);
        if with_key {
            input
                .environment
                .stable_source_keys
                .push(ProjectStableSourceKeyInput {
                    source: id,
                    key: source_key(),
                });
        }
        input.options.entry_policy = EntryPolicy::Optional;
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        output.checked.unwrap()
    }

    #[test]
    fn source_owner_ignores_session_source_ids_and_rejects_missing_or_conflicting_keys() {
        let left = checked_with_source(etas_core::SourceId(0), true);
        let right = checked_with_source(etas_core::SourceId(17), true);
        let left_catalog = catalog(&left);
        let right_catalog = catalog(&right);
        let left_owner = left_catalog
            .residual_source_owner(application(&left).owner())
            .unwrap();
        let right_owner = right_catalog
            .residual_source_owner(application(&right).owner())
            .unwrap();
        assert_eq!(left_owner, right_owner);
        let missing = checked_with_source(etas_core::SourceId(0), false);
        assert!(
            matches!(catalog(&missing).residual_source_owner(application(&missing).owner()),
            Err(ProjectionError::MissingIdentity(reason)) if reason.contains("stable source owner"))
        );

        let mut environment = crate::ProjectEnvironmentInput::default();
        let original_fingerprint = environment.canonical_environment_fingerprint();
        environment
            .stable_source_keys
            .push(ProjectStableSourceKeyInput {
                source: etas_core::SourceId(1),
                key: source_key(),
            });
        assert_ne!(
            original_fingerprint,
            environment.canonical_environment_fingerprint()
        );
        environment
            .stable_source_keys
            .push(ProjectStableSourceKeyInput {
                source: etas_core::SourceId(2),
                key: source_key(),
            });
        assert_eq!(
            environment.stable_source_key(etas_core::SourceId(1)),
            Err(crate::SourcePackageOriginError::Conflict)
        );
        environment.stable_source_keys[1].source = etas_core::SourceId(1);
        assert_eq!(
            environment.stable_source_key(etas_core::SourceId(1)),
            Err(crate::SourcePackageOriginError::Ambiguous)
        );
    }

    fn catalog(checked: &crate::CheckedProgram) -> DefinitionCatalog {
        let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([42; 32]));
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(checked, package).unwrap(),
            standard_package: package,
            external_definitions: BTreeMap::new(),
        };
        DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default()).unwrap()
    }

    fn application(checked: &crate::CheckedProgram) -> CheckedCallableApplication {
        let owner = checked
            .items
            .iter()
            .find_map(|(id, item)| match item {
                etas_hir::HirItem::Flow(flow)
                    if checked.symbols.get(flow.symbol).unwrap().name == "both" =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap();
        let boolean = checked
            .type_store
            .iter()
            .find_map(|(id, ty)| (ty == &Type::Primitive(PrimitiveType::Bool)).then_some(id))
            .unwrap();
        checked
            .checked_callable_application(
                owner,
                &[("T".into(), boolean)],
                &[(
                    "E".into(),
                    EffectRowRef {
                        effects: vec![],
                        tail: None,
                    },
                )],
            )
            .unwrap()
    }

    #[test]
    fn checked_application_projects_and_applies_stable_residual_binders() {
        let mut checked = checked();
        let original_catalog = catalog(&checked);
        let original_application = application(&checked);
        let source_owner = original_catalog
            .residual_source_owner(original_application.owner())
            .unwrap();
        let bindings = original_catalog
            .residual_application_bindings(
                &original_application,
                DescriptorLimits::default(),
                source_owner,
            )
            .unwrap();
        assert_eq!(bindings.len(), 2);
        let descriptor_type =
            GenericParameterId::from_source_owner(source_owner, 0, GenericParameterKind::Type);
        let descriptor_effect =
            GenericParameterId::from_source_owner(source_owner, 1, GenericParameterKind::EffectRow);
        let (&type_parameter, &type_argument) = bindings
            .iter()
            .find(|(_, value)| matches!(value, GenericArgumentIdentity::Type(_)))
            .unwrap();
        let (&effect_parameter, &effect_argument) = bindings
            .iter()
            .find(|(_, value)| matches!(value, GenericArgumentIdentity::EffectRow(_)))
            .unwrap();
        assert_eq!(type_parameter, descriptor_type);
        assert_eq!(effect_parameter, descriptor_effect);
        let reference = ResidualObligationRef::new(
            ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([11; 32])),
            BTreeMap::from([
                (
                    type_parameter,
                    GenericArgumentIdentity::TypeParameter(type_parameter),
                ),
                (
                    effect_parameter,
                    GenericArgumentIdentity::EffectRowParameter(effect_parameter),
                ),
            ]),
        )
        .unwrap();
        let owner = original_application.owner();
        drop(original_catalog);
        drop(original_application);
        let data = Arc::make_mut(&mut checked);
        Arc::make_mut(&mut data.effects)
            .item_effects
            .get_mut(&owner)
            .unwrap()
            .residual_checks
            .insert(reference.clone());
        let catalog = std::sync::Arc::new(catalog(&checked));
        let application = application(&checked);
        let rebound = catalog
            .residual_application_bindings(&application, DescriptorLimits::default(), source_owner)
            .unwrap();
        assert_eq!(bindings, rebound);
        let behavior = application
            .applied_behavior_with_residual_bindings(
                DescriptorLimits::default(),
                None,
                Some(&rebound),
            )
            .unwrap();
        let applied = behavior.summary().residual_checks.iter().next().unwrap();
        assert_eq!(applied.obligation(), reference.obligation());
        assert_eq!(applied.substitutions()[&type_parameter], type_argument);
        assert_eq!(applied.substitutions()[&effect_parameter], effect_argument);
        let frozen = catalog
            .freeze_callable_application(&application, DescriptorLimits::default())
            .unwrap();
        let frozen_reference = frozen
            .contract()
            .applied_behavior()
            .unwrap()
            .summary()
            .residual_checks
            .iter()
            .next()
            .unwrap();
        assert_eq!(
            frozen_reference.substitutions()[&type_parameter],
            type_argument
        );
        assert_eq!(
            frozen_reference.substitutions()[&effect_parameter],
            effect_argument
        );
        let foreign = self::application(&self::checked());
        assert!(
            catalog
                .residual_application_bindings(&foreign, DescriptorLimits::default(), source_owner)
                .is_err()
        );
    }

    #[test]
    fn checked_application_rejects_an_unbound_residual_binder() {
        let mut checked = checked();
        let owner = application(&checked).owner();
        let unknown =
            GenericParameterId::from_canonical_fingerprint(CodeFingerprint::from_bytes([91; 32]));
        let reference = ResidualObligationRef::new(
            ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([92; 32])),
            BTreeMap::from([(unknown, GenericArgumentIdentity::TypeParameter(unknown))]),
        )
        .unwrap();
        let data = Arc::make_mut(&mut checked);
        Arc::make_mut(&mut data.effects)
            .item_effects
            .get_mut(&owner)
            .unwrap()
            .residual_checks
            .insert(reference);
        let catalog = catalog(&checked);
        let application = application(&checked);
        let source_owner = catalog.residual_source_owner(application.owner()).unwrap();
        let bindings = catalog
            .residual_application_bindings(&application, DescriptorLimits::default(), source_owner)
            .unwrap();
        assert!(matches!(
            application.applied_behavior_with_residual_bindings(DescriptorLimits::default(), None, Some(&bindings)),
            Err(crate::CallableBehaviorError::InvalidApplication(reason)) if reason.contains("unbound generic parameter")
        ));
    }
}
