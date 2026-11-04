use super::*;
use etas_package_metadata::{
    ExistentialMemberWitness, ExistentialValueNode, ExistentialWitnessTarget,
};

impl Builder<'_, '_> {
    pub(super) fn reserve_object(
        &mut self,
        instance: NativeObjectInstance,
    ) -> Result<u32, PackageMetadataError> {
        if let Some(index) = self.native_objects.get(&instance) {
            return Ok(*index);
        }
        if self.objects.len() + self.values.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES
        {
            return Err(invalid(
                "value evidence graph exceeds the metadata node budget",
            ));
        }
        let index = u32::try_from(self.objects.len())
            .map_err(|_| invalid("object evidence index exceeds u32"))?;
        self.native_objects.insert(instance, index);
        self.objects.push(None);
        self.pending.push(Pending::Object(index, instance));
        Ok(index)
    }

    pub(super) fn object_node(
        &mut self,
        instance: NativeObjectInstance,
    ) -> Result<Option<ExistentialValueNode>, PackageMetadataError> {
        let checked = self.projection.checked;
        let fact = checked
            .types
            .existential_packs
            .get(&instance.pack)
            .ok_or_else(|| invalid("object evidence has no checked packing fact"))?;
        let view = checked.hir.view();
        let owner = *view
            .index()
            .expr_item
            .get(&instance.pack)
            .ok_or_else(|| invalid("object evidence has no checked producer owner"))?;
        let parameters = match checked.types.item_signatures.get(&owner) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                signature.generic_params.as_slice()
            }
            Some(ItemSignature::Agent(signature)) => signature
                .callable()
                .map_or(&[][..], |signature| signature.generic_params.as_slice()),
            Some(ItemSignature::TopLevelLet(_)) => &[],
            None => return Err(invalid("object evidence has no checked lexical signature")),
        };
        let captured = self.captured_projection(owner, parameters, instance.environment)?;
        let environment = checked
            .effects
            .callable_environments
            .get(&instance.environment)
            .ok_or_else(|| invalid("object evidence has no checked capture environment"))?;
        let mut sources = BTreeSet::new();
        for origin in environment
            .parameters
            .get(&etas_effects::ParameterCallTarget::Interface(0))
            .ok_or_else(|| invalid("object evidence has no checked producer origins"))?
        {
            let Some((source, _)) = self.checked_origin(*origin)? else {
                return Ok(None);
            };
            sources.insert(source);
        }
        let Some(payload_sources) =
            self.sources(&sources, instance.scope, Some(instance.environment))?
        else {
            return Ok(None);
        };
        let mut witnesses = Vec::with_capacity(fact.witnesses.len());
        for witness in &fact.witnesses {
            let member = match &witness.member {
                etas_types::SpecMethodIdentity::Source(symbol) => checked
                    .hir
                    .symbols
                    .get(*symbol)
                    .map(|symbol| symbol.name.clone())
                    .ok_or_else(|| invalid("object member has no checked declaration"))?,
                etas_types::SpecMethodIdentity::External { path, .. } => path
                    .last()
                    .cloned()
                    .ok_or_else(|| invalid("object member has no external declaration identity"))?,
            };
            let target = match &witness.target {
                etas_types::method::MethodTarget::BoundSpec { .. } => {
                    ExistentialWitnessTarget::Bound
                }
                etas_types::method::MethodTarget::Source {
                    symbol, callable, ..
                } => {
                    let method = self
                        .identities
                        .and_then(|identities| identities.method(*symbol))
                        .cloned()
                        .ok_or_else(|| {
                            invalid("object implementation requires its canonical method identity")
                        })?;
                    let value = self.reserve(
                        Instance {
                            unit: EffectUnit::Item(*callable),
                            environment: None,
                        },
                        instance.scope,
                    )?;
                    ExistentialWitnessTarget::Implementation { method, value }
                }
                etas_types::method::MethodTarget::External { identity } => {
                    let mut graphs = checked
                        .effects
                        .metadata_callable_graphs
                        .iter()
                        .filter(|(_, graph)| graph.implementation.as_ref() == Some(identity));
                    let (Some((key, graph)), None) = (graphs.next(), graphs.next()) else {
                        return Err(invalid(
                            "object witness has no unique checked external graph",
                        ));
                    };
                    let [CallableValueSource::Value(value)] = graph.graph.sources.as_slice() else {
                        return Err(invalid(
                            "external object witness has no owned callable root",
                        ));
                    };
                    let value = self.reserve_foreign(ForeignInstance {
                        source: etas_effects::MetadataCallableSource {
                            graph: *key,
                            value: *value,
                            environment: None,
                            analysis: etas_effects::CallableAnalysisKind::CheckedRevision,
                        },
                        root: instance.scope,
                    })?;
                    ExistentialWitnessTarget::Implementation {
                        method: identity.clone(),
                        value,
                    }
                }
                _ => {
                    return Err(invalid(
                        "object witness requires a checked portable implementation graph",
                    ));
                }
            };
            witnesses.push(ExistentialMemberWitness {
                bound: etas_package_metadata::SpecBound {
                    spec: self.projection.checked_spec_path(&witness.bound.spec)?,
                    args: witness
                        .bound
                        .args
                        .iter()
                        .map(|arg| captured.metadata_type(self.projection, *arg))
                        .collect::<Result<_, _>>()?,
                },
                member,
                target,
            });
        }
        Ok(Some(ExistentialValueNode {
            parent_scope: instance.scope,
            generic_bindings: captured.bindings.clone(),
            object_type: captured.metadata_type(self.projection, fact.object_type)?,
            source_type: captured.metadata_type(self.projection, fact.source_type)?,
            payload_sources,
            witnesses,
        }))
    }
}
