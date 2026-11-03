use super::*;
use etas_effects::{ParameterCallTarget, facts::CallableEnvironmentId};
use etas_package_metadata::{
    CallableRequestTarget, SpecMethodGenericArgument, SpecMethodInvocation,
};

impl ProjectMetadataProjection<'_> {
    pub(in crate::package_metadata) fn callable_request_target(
        &self,
        target: ParameterCallTarget,
        labels: &BTreeMap<ParameterCallTarget, String>,
    ) -> Result<CallableRequestTarget, PackageMetadataError> {
        match target {
            ParameterCallTarget::SpecMethod { call, environment } => self
                .spec_method_invocation(call, environment)
                .map(|method| CallableRequestTarget::SpecMethod(Box::new(method))),
            ParameterCallTarget::MetadataSpecMethod {
                invocation,
                environment,
            } => {
                let environment = environment.ok_or_else(|| {
                    invalid("exported method evidence has no checked invocation bindings")
                })?;
                self.external_method_invocation(invocation, environment)
                    .map(|method| CallableRequestTarget::SpecMethod(Box::new(method)))
            }
            _ => labels
                .get(&target)
                .cloned()
                .map(CallableRequestTarget::Parameter)
                .ok_or_else(|| invalid("call evidence has no checked exported parameter binding")),
        }
    }

    pub(crate) fn external_method_invocation(
        &self,
        invocation: etas_code::CodeFingerprint,
        environment: CallableEnvironmentId,
    ) -> Result<SpecMethodInvocation, PackageMetadataError> {
        use etas_effects::facts::{CallableGenericParameter, CallableGenericValue};
        let definition = self
            .checked
            .effects
            .method_evidence
            .get(&invocation)
            .ok_or_else(|| invalid("exported method evidence has no checked descriptor"))?;
        let snapshot = self
            .checked
            .effects
            .callable_environments
            .get(&environment)
            .ok_or_else(|| invalid("exported method evidence binding environment is missing"))?;
        let builder = Builder::new(self, snapshot.owner);
        let mut captured = builder.generic_context(Some(snapshot.owner))?;
        captured.owner_type_names.clear();
        for (position, (name, kind)) in definition.generics.iter().enumerate() {
            let value = snapshot
                .generics
                .get(&CallableGenericParameter::Graph {
                    graph: invocation,
                    scope: CallableValueScope::Root,
                    position: position as u32,
                })
                .ok_or_else(|| {
                    invalid("exported metadata method has an incomplete generic environment")
                })?;
            match (kind, value) {
                (etas_types::CallableGenericParamKind::Type, CallableGenericValue::Type(ty)) => {
                    captured.owner_type_names.insert(name.clone());
                    let mut ty = self.metadata_type(*ty)?;
                    normalize_generic_type(&mut ty, &captured.type_names);
                    captured.types.insert(name.clone(), ty);
                }
                (
                    etas_types::CallableGenericParamKind::Effect,
                    CallableGenericValue::EffectRow(row),
                ) => {
                    let row = self.effect_row_metadata(
                        row,
                        &captured.type_names,
                        &captured.effect_names,
                    )?;
                    captured.rows.insert(name.clone(), row);
                }
                _ => {
                    return Err(invalid(
                        "exported metadata method generic kind is inconsistent",
                    ));
                }
            }
        }
        let method = &definition.invocation;
        Ok(SpecMethodInvocation {
            receiver: captured.metadata_type(self, method.receiver)?,
            spec: method.spec.path.clone(),
            spec_args: method
                .spec_args
                .iter()
                .map(|ty| captured.metadata_type(self, *ty))
                .collect::<Result<_, _>>()?,
            method: method.method.clone(),
            arity: method.arity,
            generic_arguments: method
                .generic_arguments
                .iter()
                .map(|argument| {
                    Ok(match argument {
                        etas_effects::ExternalSpecMethodGenericArgument::Type(ty) => {
                            SpecMethodGenericArgument::Type(Box::new(
                                captured.metadata_type(self, *ty)?,
                            ))
                        }
                        etas_effects::ExternalSpecMethodGenericArgument::EffectRow(row) => {
                            SpecMethodGenericArgument::EffectRow(captured.effect_row(
                                metadata_effect_row_ref(row, &self.checked.type_store)?,
                            )?)
                        }
                    })
                })
                .collect::<Result<_, PackageMetadataError>>()?,
            callable_arguments: self.method_callable_arguments(snapshot)?,
        })
    }

    fn spec_method_invocation(
        &self,
        call: etas_hir::HirExprId,
        environment: CallableEnvironmentId,
    ) -> Result<SpecMethodInvocation, PackageMetadataError> {
        let Some(etas_hir::HirExpr::SpecMethodCall {
            receiver,
            spec_path,
            spec_args,
            method,
            args,
            ..
        }) = self.checked.hir.exprs.get(call)
        else {
            return Err(invalid(
                "method evidence has no checked selection expression",
            ));
        };
        let etas_hir::ResolveResult::Resolved(symbol) = spec_path.resolution else {
            return Err(invalid("method evidence has no resolved spec identity"));
        };
        let symbol = self
            .checked
            .hir
            .canonical_source_symbol(symbol)
            .ok_or_else(|| invalid("method evidence has no canonical spec symbol"))?;
        let spec = self
            .symbol_canonical_path(symbol)
            .ok_or_else(|| invalid("method evidence spec has no exportable identity"))?;
        let declaration = self
            .checked
            .types
            .spec_signatures
            .get(&symbol)
            .and_then(|spec| {
                spec.methods
                    .iter()
                    .find(|candidate| candidate.name == *method)
            })
            .ok_or_else(|| invalid("method evidence has no checked method declaration"))?;
        let signature = declaration
            .signature_in(&self.checked.types)
            .ok_or_else(|| invalid("method evidence has no checked callable signature"))?;
        let lexical_owner = *self
            .checked
            .hir
            .view()
            .index()
            .expr_item
            .get(&call)
            .ok_or_else(|| invalid("method evidence has no lexical owner"))?;
        let parameters = self
            .checked
            .types
            .item_signatures
            .get(&lexical_owner)
            .and_then(|signature| match signature {
                ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => {
                    Some(signature.generic_params.as_slice())
                }
                ItemSignature::Agent(signature) => signature
                    .callable()
                    .map(|signature| signature.generic_params.as_slice()),
                _ => None,
            })
            .ok_or_else(|| invalid("method evidence has no lexical generic signature"))?;
        let snapshot = self
            .checked
            .effects
            .callable_environments
            .get(&environment)
            .ok_or_else(|| invalid("method evidence has no checked binding environment"))?;
        let builder = Builder::new(self, snapshot.owner);
        let captured = builder.captured_projection(lexical_owner, parameters, environment)?;
        let receiver = *self
            .checked
            .types
            .expr_types
            .get(receiver)
            .ok_or_else(|| invalid("method evidence receiver has no checked type"))?;
        let spec_args = spec_args
            .iter()
            .map(|ty| {
                let ty =
                    *self.checked.types.type_refs.get(ty).ok_or_else(|| {
                        invalid("method evidence spec argument has no checked type")
                    })?;
                captured.metadata_type(self, ty)
            })
            .collect::<Result<_, _>>()?;
        let arguments = self.checked.types.generic_instantiations.get(&call);
        let mut generic_arguments = Vec::new();
        for (position, parameter) in signature.generic_params.iter().enumerate() {
            if Some(position as u32) == declaration.receiver_param {
                continue;
            }
            let arguments = arguments
                .ok_or_else(|| invalid("method evidence generic arguments are missing"))?;
            generic_arguments.push(match parameter.kind {
                etas_types::CallableGenericParamKind::Type => {
                    let ty = *arguments
                        .type_bindings
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, ty)| ty)
                        .ok_or_else(|| invalid("method evidence type argument is missing"))?;
                    SpecMethodGenericArgument::Type(Box::new(captured.metadata_type(self, ty)?))
                }
                etas_types::CallableGenericParamKind::Effect => {
                    let row = arguments
                        .effect_row_bindings
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, row)| row)
                        .ok_or_else(|| invalid("method evidence effect argument is not solved"))?;
                    let row = metadata_effect_row_ref(row, &self.checked.type_store)?;
                    SpecMethodGenericArgument::EffectRow(captured.effect_row(row)?)
                }
            });
        }
        let callable_arguments = self.method_callable_arguments(snapshot)?;
        Ok(SpecMethodInvocation {
            receiver: captured.metadata_type(self, receiver)?,
            spec,
            spec_args,
            method: method.clone(),
            arity: u32::try_from(args.len() + 1)
                .map_err(|_| invalid("method arity exceeds u32"))?,
            generic_arguments,
            callable_arguments,
        })
    }

    fn method_callable_arguments(
        &self,
        snapshot: &etas_effects::facts::CallableEnvironment,
    ) -> Result<Vec<etas_package_metadata::SpecMethodCallableArgument>, PackageMetadataError> {
        let mut callable_arguments = Vec::new();
        for (target, origins) in &snapshot.parameters {
            let ParameterCallTarget::Interface(position) = target else {
                return Err(invalid(
                    "method evidence callback has no checked input position",
                ));
            };
            let mut builder = Builder::new(self, snapshot.owner);
            let mut sources = Vec::new();
            for origin in origins {
                let (source, environment) = builder
                    .checked_origin(*origin)?
                    .ok_or_else(|| invalid("method evidence callback has no exportable origin"))?;
                sources.extend(
                    builder
                        .sources(&[source].into(), CallableValueScope::Root, environment)?
                        .ok_or_else(|| {
                            invalid("method evidence callback sources are incomplete")
                        })?,
                );
            }
            sources.sort();
            sources.dedup();
            let graph = builder
                .finish(sources)?
                .ok_or_else(|| invalid("method evidence callback graph is incomplete"))?;
            callable_arguments.push(etas_package_metadata::SpecMethodCallableArgument {
                position: *position,
                sources: std::sync::Arc::unwrap_or_clone(graph.graph),
            });
        }
        Ok(callable_arguments)
    }
}
