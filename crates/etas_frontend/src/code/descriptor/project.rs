use std::collections::HashMap;

use etas_code::{DefinitionKey, descriptor::*};
use etas_types::{CallableSignature, EffectRowRef, Type, TypeId};

use super::ProjectionError;
use crate::{
    CheckedProgramData,
    code::identity::{DefinitionIdentityIndex, GenericDeclaration, TypeDeclaration},
};

#[derive(Clone, Copy, Debug)]
pub struct ProjectedSignature {
    pub ty: TypeInfoRef,
}

/// Session-local correspondence, never serialized as a portable type identity.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct NativeTypeOrigin {
    pub local: TypeId,
    pub scope: GenericOwner,
    pub descriptor: TypeInfoRef,
}

pub struct NativeDescriptorProjection {
    pub graph: DescriptorGraph,
    pub type_origins: Vec<NativeTypeOrigin>,
}

pub(super) struct Scope {
    pub owner: GenericOwner,
    pub binders: HashMap<String, GenericParameterRef>,
    parameters: Vec<GenericDeclaration>,
}

pub(super) enum Work {
    Type {
        source: TypeId,
        scope: usize,
        target: TypeInfoRef,
    },
    Declaration {
        source: Option<TypeId>,
        declaration: TypeDeclaration,
        scope: usize,
        target: TypeInfoRef,
    },
    Standard {
        source: etas_std::StdType,
        scope: usize,
        module: Vec<String>,
        target: TypeInfoRef,
    },
    Row {
        source: EffectRowRef,
        scope: usize,
        target: EffectRowInfoRef,
    },
    Bounds {
        declaration: GenericDeclaration,
        scope: usize,
        target: GenericParameterRef,
    },
}

/// Projects the reachable checked graph only. Scope is part of memo identity:
/// the interner can reuse Named("T") for unrelated declaration binders.
pub struct DefinitionDescriptorProjector<'a> {
    pub(super) checked: super::DescriptorInput<'a>,
    pub(super) identities: &'a DefinitionIdentityIndex,
    pub(super) limits: DescriptorLimits,
    pub(super) scopes: Vec<Scope>,
    pub(super) types: Vec<Option<TypeInfo>>,
    pub(super) rows: Vec<Option<EffectRowInfo>>,
    pub(super) binders: Vec<GenericParameterInfo>,
    pub(super) traces: Vec<Option<TraceInfo>>,
    pub(super) work: Vec<Work>,
    memo: HashMap<(TypeId, usize), TypeInfoRef>,
    declarations: HashMap<DefinitionKey, TypeInfoRef>,
    edges: usize,
    text_bytes: usize,
}

impl<'a> DefinitionDescriptorProjector<'a> {
    pub fn new(
        checked: &'a CheckedProgramData,
        identities: &'a DefinitionIdentityIndex,
        limits: DescriptorLimits,
    ) -> Self {
        Self::from_input(checked.into(), identities, limits)
    }

    pub(crate) fn from_input(
        checked: super::DescriptorInput<'a>,
        identities: &'a DefinitionIdentityIndex,
        limits: DescriptorLimits,
    ) -> Self {
        Self {
            checked,
            identities,
            limits,
            scopes: Vec::new(),
            types: Vec::new(),
            rows: Vec::new(),
            binders: Vec::new(),
            traces: Vec::new(),
            work: Vec::new(),
            memo: HashMap::new(),
            declarations: HashMap::new(),
            edges: 0,
            text_bytes: 0,
        }
    }

    pub fn project_signature(
        &mut self,
        owner: GenericOwner,
        signature: &CallableSignature,
        parameter_names: &[Option<String>],
        declared_output: Option<TypeId>,
    ) -> Result<ProjectedSignature, ProjectionError> {
        if signature.params.len() != parameter_names.len() {
            return Err(ProjectionError::Invalid(
                "checked callable parameter names and types have different arity".into(),
            ));
        }
        self.ensure_edges(signature.params.len().saturating_add(2))?;
        let new_binders = if self.scopes.iter().any(|scope| scope.owner == owner) {
            0
        } else {
            signature.generic_params.len()
        };
        self.ensure_nodes(new_binders.saturating_add(1))?;
        let generics = signature
            .generic_params
            .iter()
            .map(|parameter| GenericDeclaration {
                name: parameter.name.clone(),
                effect: parameter.kind == etas_types::CallableGenericParamKind::Effect,
                bounds: parameter
                    .bounds
                    .iter()
                    .cloned()
                    .map(crate::code::identity::BoundDeclaration::Checked)
                    .collect(),
            })
            .collect::<Vec<_>>();
        let scope = self.scope(owner, &generics)?;
        let info = self.signature(signature, parameter_names, declared_output, scope)?;
        let ty = self.reserve_type()?;
        self.types[ty.0 as usize] = Some(TypeInfo::Function(info));
        Ok(ProjectedSignature { ty })
    }

    pub fn project_type_declaration(
        &mut self,
        symbol: etas_hir::SymbolId,
    ) -> Result<TypeInfoRef, ProjectionError> {
        let declaration = self
            .identities
            .type_declarations
            .get(&symbol)
            .ok_or(ProjectionError::MissingDeclaration(symbol))?
            .clone();
        self.declaration_ref(declaration.checked_type, declaration)
    }

    pub fn project_closed_types(
        &mut self,
        owner: GenericOwner,
        types: &[TypeId],
    ) -> Result<Vec<TypeInfoRef>, ProjectionError> {
        self.ensure_edges(types.len())?;
        let scope = self.scope(owner, &[])?;
        types.iter().map(|ty| self.type_ref(*ty, scope)).collect()
    }

    pub fn project_closed_rows(
        &mut self,
        owner: GenericOwner,
        rows: &[EffectRowRef],
    ) -> Result<Vec<EffectRowInfoRef>, ProjectionError> {
        self.ensure_edges(rows.len())?;
        let scope = self.scope(owner, &[])?;
        rows.iter()
            .map(|row| {
                if let Some(tail) = &row.tail {
                    return Err(ProjectionError::UnresolvedBinder(tail.clone()));
                }
                self.row_ref(row.clone(), scope)
            })
            .collect()
    }

    pub(crate) fn project_scoped_types(
        &mut self,
        owner: &GenericOwner,
        types: &[TypeId],
    ) -> Result<Vec<TypeInfoRef>, ProjectionError> {
        self.ensure_edges(types.len())?;
        let scope = self
            .scopes
            .iter()
            .position(|scope| &scope.owner == owner)
            .ok_or_else(|| ProjectionError::MissingIdentity("callable generic scope".into()))?;
        types.iter().map(|ty| self.type_ref(*ty, scope)).collect()
    }

    pub(super) fn signature(
        &mut self,
        signature: &CallableSignature,
        names: &[Option<String>],
        declared: Option<TypeId>,
        scope: usize,
    ) -> Result<SignatureInfo, ProjectionError> {
        if let Some(row) = &signature.effects {
            self.ensure_row(row)?;
        }
        let mut parameters = Vec::with_capacity(signature.params.len());
        for (ty, name) in signature.params.iter().zip(names) {
            if let Some(name) = name {
                self.text(name)?;
            }
            parameters.push(ParameterInfo {
                name: name.clone(),
                ty: self.type_ref(*ty, scope)?,
            });
        }
        let mut generics = self.scopes[scope]
            .binders
            .values()
            .copied()
            .collect::<Vec<_>>();
        generics.sort_by_key(|parameter| self.binders[parameter.0 as usize].position);
        Ok(SignatureInfo {
            generics,
            parameters,
            output: self.type_ref(signature.output, scope)?,
            declared_output: declared.map(|ty| self.type_ref(ty, scope)).transpose()?,
            effects: signature
                .effects
                .as_ref()
                .map(|row| self.row_ref(row.clone(), scope))
                .transpose()?,
        })
    }

    pub(super) fn scope(
        &mut self,
        owner: GenericOwner,
        parameters: &[GenericDeclaration],
    ) -> Result<usize, ProjectionError> {
        if let Some((scope, _)) = self
            .scopes
            .iter()
            .enumerate()
            .find(|(_, scope)| scope.owner == owner)
        {
            if self.scopes[scope].parameters != parameters {
                return Err(ProjectionError::Invalid(
                    "one definition identity has conflicting generic binders".into(),
                ));
            }
            return Ok(scope);
        }
        self.ensure_nodes(parameters.len())?;
        let scope = self.scopes.len();
        self.scopes.push(Scope {
            owner: owner.clone(),
            binders: HashMap::new(),
            parameters: parameters.to_vec(),
        });
        for (position, parameter) in parameters.iter().enumerate() {
            self.node()?;
            self.text(&parameter.name)?;
            let target = GenericParameterRef(self.binders.len() as u32);
            if self.scopes[scope]
                .binders
                .insert(parameter.name.clone(), target)
                .is_some()
            {
                return Err(ProjectionError::Invalid(
                    "duplicate checked generic binder name".into(),
                ));
            }
            self.binders.push(GenericParameterInfo {
                owner: owner.clone(),
                position: position as u32,
                name: parameter.name.clone(),
                kind: if parameter.effect {
                    GenericParameterKind::EffectRow
                } else {
                    GenericParameterKind::Type
                },
                bounds: Vec::new(),
            });
            self.work.push(Work::Bounds {
                declaration: parameter.clone(),
                scope,
                target,
            });
        }
        Ok(scope)
    }

    pub(super) fn type_ref(
        &mut self,
        source: TypeId,
        scope: usize,
    ) -> Result<TypeInfoRef, ProjectionError> {
        self.edge()?;
        if let Some(reference) = self.memo.get(&(source, scope)) {
            return Ok(*reference);
        }
        let data = self
            .checked
            .type_store
            .get(source)
            .ok_or(ProjectionError::MissingType(source))?;
        let declaration = match data {
            Type::Nominal(data) => self
                .identities
                .types
                .get(&source)
                .or_else(|| self.identities.type_names.get(&data.name)),
            Type::Enum(data) => self
                .identities
                .types
                .get(&source)
                .or_else(|| self.identities.type_names.get(&data.name)),
            Type::Named(data) if !self.scopes[scope].binders.contains_key(&data.name) => {
                self.identities.type_names.get(&data.name)
            }
            _ => None,
        }
        .cloned();
        let reference = if let Some(declaration) = declaration {
            self.declaration_ref(Some(source), declaration)?
        } else {
            let target = self.reserve_type()?;
            self.work.push(Work::Type {
                source,
                scope,
                target,
            });
            target
        };
        self.memo.insert((source, scope), reference);
        Ok(reference)
    }

    pub(super) fn declaration_ref(
        &mut self,
        source: Option<TypeId>,
        declaration: TypeDeclaration,
    ) -> Result<TypeInfoRef, ProjectionError> {
        if let Some(reference) = self.declarations.get(&declaration.identity) {
            return Ok(*reference);
        }
        let target = self.reserve_type()?;
        self.declarations
            .insert(declaration.identity.clone(), target);
        let scope = self.scope(
            GenericOwner::Declaration(declaration.identity.clone()),
            &declaration.parameters,
        )?;
        if let Some(source) = declaration.checked_type.or(source)
            && matches!(
                self.checked.type_store.get(source),
                Some(Type::Nominal(_) | Type::Enum(_))
            )
        {
            self.memo.insert((source, scope), target);
        }
        self.work.push(Work::Declaration {
            source: declaration.checked_type.or(source),
            declaration,
            scope,
            target,
        });
        Ok(target)
    }

    pub(super) fn standard_ref(
        &mut self,
        source: etas_std::StdType,
        scope: usize,
        module: &[String],
    ) -> Result<TypeInfoRef, ProjectionError> {
        self.edge()?;
        if let etas_std::StdType::Named(name) = &source {
            let declaration = self.standard_declaration(name, module)?;
            return self.declaration_ref(None, declaration);
        }
        let target = self.reserve_type()?;
        self.work.push(Work::Standard {
            source,
            scope,
            module: module.to_vec(),
            target,
        });
        Ok(target)
    }

    pub(super) fn row_ref(
        &mut self,
        source: EffectRowRef,
        scope: usize,
    ) -> Result<EffectRowInfoRef, ProjectionError> {
        self.edge()?;
        self.node()?;
        let target = EffectRowInfoRef(self.rows.len() as u32);
        self.rows.push(None);
        self.work.push(Work::Row {
            source,
            scope,
            target,
        });
        Ok(target)
    }

    pub fn finish(self) -> Result<DescriptorGraph, ProjectionError> {
        self.finish_native().map(|projection| projection.graph)
    }

    pub fn finish_native(mut self) -> Result<NativeDescriptorProjection, ProjectionError> {
        self.materialize_pending()?;
        let mut type_origins = self
            .memo
            .iter()
            .map(|((local, scope), descriptor)| {
                let scope = self.scopes.get(*scope).ok_or_else(|| {
                    ProjectionError::Invalid("native type origin has no generic scope".into())
                })?;
                Ok(NativeTypeOrigin {
                    local: *local,
                    scope: scope.owner.clone(),
                    descriptor: *descriptor,
                })
            })
            .collect::<Result<Vec<_>, ProjectionError>>()?;
        type_origins.sort_by(|a, b| (&a.scope, a.local).cmp(&(&b.scope, b.local)));
        let input = DescriptorGraphInput {
            types: self
                .types
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    ProjectionError::Invalid("descriptor type was not materialized".into())
                })?,
            rows: self
                .rows
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    ProjectionError::Invalid("descriptor row was not materialized".into())
                })?,
            binders: self.binders,
            traces: self
                .traces
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    ProjectionError::Invalid("descriptor trace was not materialized".into())
                })?,
        };
        Ok(NativeDescriptorProjection {
            graph: DescriptorGraph::new(input, self.limits)?,
            type_origins,
        })
    }

    pub(super) fn materialize_pending(&mut self) -> Result<(), ProjectionError> {
        while let Some(work) = self.work.pop() {
            match work {
                Work::Type {
                    source,
                    scope,
                    target,
                } => {
                    self.types[target.0 as usize] = Some(self.project_type(source, scope)?);
                }
                Work::Declaration {
                    source,
                    declaration,
                    scope,
                    target,
                } => {
                    self.types[target.0 as usize] =
                        Some(self.project_declaration(source, declaration, scope)?);
                }
                Work::Standard {
                    source,
                    scope,
                    module,
                    target,
                } => {
                    self.types[target.0 as usize] =
                        Some(self.project_standard(source, scope, &module)?);
                }
                Work::Row {
                    source,
                    scope,
                    target,
                } => {
                    self.rows[target.0 as usize] = Some(self.project_row(source, scope)?);
                }
                Work::Bounds {
                    declaration,
                    scope,
                    target,
                } => {
                    self.binders[target.0 as usize].bounds =
                        self.project_bounds(declaration.bounds, scope)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn reserve_type(&mut self) -> Result<TypeInfoRef, ProjectionError> {
        self.node()?;
        let target = TypeInfoRef(self.types.len() as u32);
        self.types.push(None);
        Ok(target)
    }
    pub(super) fn node(&self) -> Result<(), ProjectionError> {
        self.ensure_nodes(1)
    }
    pub(super) fn ensure_nodes(&self, additional: usize) -> Result<(), ProjectionError> {
        let count = self
            .types
            .len()
            .saturating_add(self.rows.len())
            .saturating_add(self.binders.len());
        let count = count.saturating_add(self.traces.len());
        if additional
            > self
                .limits
                .max_nodes
                .min(u32::MAX as usize)
                .saturating_sub(count)
        {
            return Err(ProjectionError::Limit {
                resource: "nodes",
                limit: self.limits.max_nodes,
            });
        }
        Ok(())
    }
    pub(super) fn ensure_edges(&self, additional: usize) -> Result<(), ProjectionError> {
        if additional > self.limits.max_edges.saturating_sub(self.edges) {
            return Err(ProjectionError::Limit {
                resource: "edges",
                limit: self.limits.max_edges,
            });
        }
        Ok(())
    }
    pub(super) fn ensure_row(&self, row: &EffectRowRef) -> Result<(), ProjectionError> {
        let edges = row
            .effects
            .iter()
            .fold(usize::from(row.tail.is_some()), |count, effect| {
                count.saturating_add(effect.args.len()).saturating_add(1)
            });
        self.ensure_edges(edges)
    }
    pub(super) fn edge(&mut self) -> Result<(), ProjectionError> {
        self.edges = self.edges.saturating_add(1);
        if self.edges > self.limits.max_edges {
            return Err(ProjectionError::Limit {
                resource: "edges",
                limit: self.limits.max_edges,
            });
        }
        Ok(())
    }
    pub(super) fn text(&mut self, value: &str) -> Result<(), ProjectionError> {
        self.text_bytes = self.text_bytes.saturating_add(value.len());
        if self.text_bytes > self.limits.max_text_bytes {
            return Err(ProjectionError::Limit {
                resource: "text bytes",
                limit: self.limits.max_text_bytes,
            });
        }
        Ok(())
    }
}
