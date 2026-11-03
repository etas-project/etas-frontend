use std::collections::HashMap;

use crate::{
    CallableSignature, CallableSpecSatisfactionFact, CheckedIndexKind, CheckedSliceKind,
    EffectArgRef, EffectRef, EffectRowRef, ExternalCallableSpecSatisfactionFact,
    ExternalTraceSpecConformanceFact, ExternalTraceSpecConformanceTarget, FieldType,
    HandlerProducedEffects, ItemSignature, ResourceHandleFact, ResourceHandleType, SpecImplFact,
    SpecSignature, SymbolTypeFact, TraceSpecConformanceFact, TraceSpecConformanceTarget,
    TryExprTypeFact, Type, TypeConstructorId, TypeId, TypeInterner, TypeSpecSatisfactionFact,
    TypeStore,
};

pub(crate) fn remap_body_delta(
    output: &crate::BodyTypeOutput,
    base: &crate::SignatureEnvironment,
    target: &mut TypeInterner,
) -> Result<crate::TypeFacts, String> {
    if !base.accepts(output) {
        return Err("body type delta belongs to a different signature type base".to_owned());
    }
    let source = TypeStore::with_local_parts(
        base.types.clone(),
        output.types.clone(),
        output.nominal_identities.clone(),
        output.parameter_identities.clone(),
    )
    .map_err(str::to_owned)?;
    let mut remapper = TypeIdRemapper::new(&source, target);
    remapper.base_len = output.base_len;
    for index in 0..output.types.len() {
        remapper.ty(TypeId((output.base_len + index) as u32));
    }
    let mut facts = output.facts.clone();
    remap_facts(&mut facts, &mut remapper);
    if let Some(error) = remapper.error {
        return Err(error);
    }
    Ok(facts)
}

struct TypeIdRemapper<'a, 'b> {
    source: Option<&'a TypeStore>,
    substitutions: Option<&'a crate::Substitution>,
    target: &'b mut TypeInterner,
    ids: HashMap<TypeId, TypeId>,
    base_len: usize,
    error: Option<String>,
}

impl<'a, 'b> TypeIdRemapper<'a, 'b> {
    fn new(source: &'a TypeStore, target: &'b mut TypeInterner) -> Self {
        Self {
            source: Some(source),
            substitutions: None,
            target,
            ids: HashMap::new(),
            base_len: 0,
            error: None,
        }
    }

    fn ty(&mut self, ty: TypeId) -> TypeId {
        if (ty.0 as usize) < self.base_len {
            return ty;
        }
        if let Some(mapped) = self.ids.get(&ty).copied() {
            return mapped;
        }
        if let Some(substitutions) = self.substitutions {
            return match crate::substitute_type_params(
                self.target,
                ty,
                &HashMap::new(),
                substitutions,
            ) {
                Ok(mapped) => {
                    self.ids.insert(ty, mapped);
                    mapped
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    ty
                }
            };
        }
        let Some(source_ty) = self.source.and_then(|source| source.get(ty)).cloned() else {
            self.error = Some(format!("body type delta references missing type {}", ty.0));
            return ty;
        };
        let identity = self
            .source
            .and_then(|source| source.nominal_identity(ty))
            .cloned();
        if let Some(parameter) = self
            .source
            .and_then(|source| source.parameter_identity(ty))
            .cloned()
        {
            let Type::Named(name) = source_ty else {
                self.error = Some("canonical parameter has invalid type".into());
                return ty;
            };
            return match self.target.intern_canonical_parameter(parameter, name.name) {
                Ok(mapped) => {
                    self.ids.insert(ty, mapped);
                    mapped
                }
                Err(error) => {
                    self.error = Some(error.into());
                    ty
                }
            };
        }
        if let Type::Nominal(mut nominal) = source_ty.clone() {
            if let Some(identity) = identity {
                let representation = nominal.representation.take();
                let existing = self.target.canonical_nominal(&identity);
                let mapped = match existing {
                    Some(id) => id,
                    None => match self
                        .target
                        .intern_canonical_nominal(identity, Type::Nominal(nominal.clone()))
                    {
                        Ok(id) => id,
                        Err(error) => {
                            self.error = Some(error.to_owned());
                            return ty;
                        }
                    },
                };
                self.ids.insert(ty, mapped);
                nominal.representation =
                    representation.map(|representation| self.ty(representation));
                if existing.is_some() {
                    if self.target.store().get(mapped) != Some(&Type::Nominal(nominal)) {
                        self.error =
                            Some("canonical nominal identity has conflicting layouts".into());
                    }
                } else {
                    self.target.define_nominal(mapped, nominal.representation);
                }
                return mapped;
            }
            if let Some((id, _)) = self.target.store().iter().find(|(id, data)| {
                self.target.store().nominal_identity(*id).is_none()
                    && matches!(data, Type::Nominal(existing) if existing.name == nominal.name)
            }) {
                self.ids.insert(ty, id);
                return id;
            }
            let representation = nominal.representation.take();
            let mapped = self.target.intern(Type::Nominal(nominal));
            self.ids.insert(ty, mapped);
            let representation = representation.map(|representation| self.ty(representation));
            self.target.define_nominal(mapped, representation);
            return mapped;
        }
        let mapped_ty = self.type_data(source_ty);
        let mapped = if let Some(identity) = identity {
            match self.target.intern_canonical_nominal(identity, mapped_ty) {
                Ok(id) => id,
                Err(error) => {
                    self.error = Some(error.to_owned());
                    return ty;
                }
            }
        } else {
            self.target.intern(mapped_ty)
        };
        self.ids.insert(ty, mapped);
        mapped
    }

    fn constructor(&mut self, constructor: TypeConstructorId) -> TypeConstructorId {
        TypeConstructorId(self.ty(TypeId(constructor.0)).0)
    }

    fn type_data(&mut self, ty: Type) -> Type {
        match ty {
            Type::Array(inner) => Type::Array(self.ty(inner)),
            Type::List(inner) => Type::List(self.ty(inner)),
            Type::Map { key, value } => Type::Map {
                key: self.ty(key),
                value: self.ty(value),
            },
            Type::Set(inner) => Type::Set(self.ty(inner)),
            Type::Range { index } => Type::Range {
                index: self.ty(index),
            },
            Type::Slice(inner) => Type::Slice(self.ty(inner)),
            Type::Option(inner) => Type::Option(self.ty(inner)),
            Type::Result { ok, err } => Type::Result {
                ok: self.ty(ok),
                err: self.ty(err),
            },
            Type::Record(record) => Type::Record(crate::RecordType {
                fields: record
                    .fields
                    .into_iter()
                    .map(|field| FieldType {
                        name: field.name,
                        ty: self.ty(field.ty),
                    })
                    .collect(),
            }),
            Type::Tuple(elements) => {
                Type::Tuple(elements.into_iter().map(|ty| self.ty(ty)).collect())
            }
            Type::Function(mut flow) => {
                flow.input = flow.input.into_iter().map(|ty| self.ty(ty)).collect();
                flow.output = self.ty(flow.output);
                flow.effects = flow.effects.map(|row| self.effect_row(row));
                Type::Function(flow)
            }
            Type::Handler(mut handler) => {
                handler.handled = self.effect_row(handler.handled);
                handler.produced = match handler.produced {
                    HandlerProducedEffects::Infer => HandlerProducedEffects::Infer,
                    HandlerProducedEffects::Explicit(row) => {
                        HandlerProducedEffects::Explicit(self.effect_row(row))
                    }
                };
                handler.result = handler.result.map(|ty| self.ty(ty));
                Type::Handler(handler)
            }
            Type::Nominal(mut nominal) => {
                nominal.representation = nominal.representation.map(|ty| self.ty(ty));
                Type::Nominal(nominal)
            }
            Type::Applied { constructor, args } => Type::Applied {
                constructor: self.constructor(constructor),
                args: args.into_iter().map(|ty| self.ty(ty)).collect(),
            },
            Type::Refined { base, predicate } => Type::Refined {
                base: self.ty(base),
                predicate,
            },
            Type::Trust { wrapper, inner } => Type::Trust {
                wrapper,
                inner: self.ty(inner),
            },
            Type::Schema(inner) => Type::Schema(self.ty(inner)),
            Type::Message(inner) => Type::Message(self.ty(inner)),
            Type::MemorySelection(inner) => Type::MemorySelection(self.ty(inner)),
            Type::Store { key, value } => Type::Store {
                key: self.ty(key),
                value: self.ty(value),
            },
            Type::MemoryRegion(inner) => Type::MemoryRegion(self.ty(inner)),
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                    schema: self.ty(schema),
                })
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                Type::ResourceHandle(ResourceHandleType::ExternalTool {
                    signature: self.ty(signature),
                })
            }
            Type::ResourceHandle(ResourceHandleType::Other { name, args }) => {
                Type::ResourceHandle(ResourceHandleType::Other {
                    name,
                    args: args.into_iter().map(|ty| self.ty(ty)).collect(),
                })
            }
            // Body-local inference identities must not alias another body's pending proof.
            Type::Var(variable) => match u32::try_from(self.target.store().len()) {
                Ok(index) => Type::Var(crate::TypeVarId(index)),
                Err(_) => {
                    self.error = Some("merged type variable identity space is exhausted".into());
                    Type::Var(variable)
                }
            },
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Enum(_)
            | Type::Named(_)
            | Type::Prompt
            | Type::PromptPart
            | Type::MemoryPlace(_) => ty,
        }
    }

    fn callable(&mut self, signature: &mut CallableSignature) {
        for generic in &mut signature.generic_params {
            generic.subject = self.ty(generic.subject);
            for bound in &mut generic.bounds {
                for arg in &mut bound.args {
                    *arg = self.ty(*arg);
                }
            }
        }
        for param in &mut signature.params {
            *param = self.ty(*param);
        }
        signature.output = self.ty(signature.output);
        signature.effects = signature.effects.take().map(|row| self.effect_row(row));
        signature.requested_actions = signature
            .requested_actions
            .take()
            .map(|row| self.effect_row(row));
    }

    fn item_signature(&mut self, signature: &mut ItemSignature) {
        match signature {
            ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => {
                self.callable(signature)
            }
            ItemSignature::Agent(signature) => self.agent(signature),
            ItemSignature::TopLevelLet(signature) => signature.ty = self.ty(signature.ty),
        }
    }

    fn symbol_fact(&mut self, fact: &mut SymbolTypeFact) {
        match fact {
            SymbolTypeFact::Param { ty }
            | SymbolTypeFact::Local { ty, .. }
            | SymbolTypeFact::Field { ty }
            | SymbolTypeFact::Value { ty }
            | SymbolTypeFact::TopLevelLet { ty, .. } => *ty = self.ty(*ty),
            SymbolTypeFact::TypeAlias { target, params } => {
                *target = self.ty(*target);
                for param in params {
                    param.subject = self.ty(param.subject);
                    for bound in &mut param.bounds {
                        for arg in &mut bound.args {
                            *arg = self.ty(*arg);
                        }
                    }
                }
            }
            SymbolTypeFact::Flow { signature } | SymbolTypeFact::Tool { signature } => {
                self.callable(signature)
            }
            SymbolTypeFact::Agent { signature } => self.agent(signature),
            SymbolTypeFact::Type { constructor } => *constructor = self.constructor(*constructor),
            SymbolTypeFact::NominalType {
                constructor,
                params: _,
                representation,
            } => {
                *constructor = self.constructor(*constructor);
                *representation = representation.map(|ty| self.ty(ty));
            }
            SymbolTypeFact::EffectAction { signature } => self.action_signature(signature),
            SymbolTypeFact::Effect { .. } | SymbolTypeFact::Spec { .. } | SymbolTypeFact::Error => {
            }
        }
    }

    fn agent(&mut self, signature: &mut crate::AgentSignature) {
        signature.component = self.ty(signature.component);
        signature.definition_type = signature.definition_type.map(|ty| self.ty(ty));
        if let Some(entry) = &mut signature.default_entry {
            let entry = std::sync::Arc::make_mut(entry);
            entry.callable_type = self.ty(entry.callable_type);
            self.callable(&mut entry.callable);
        }
    }

    fn resource_handle_fact(&mut self, fact: &mut ResourceHandleFact) {
        match fact {
            ResourceHandleFact::MemoryRegion { schema, .. } => {
                *schema = self.ty(*schema);
            }
        }
    }

    fn action_signature(&mut self, signature: &mut crate::EffectActionSignature) {
        signature.default_handler_effects = signature
            .default_handler_effects
            .take()
            .map(|row| self.effect_row(row));
        for generic in &mut signature.generic_params {
            generic.subject = self.ty(generic.subject);
            for bound in &mut generic.bounds {
                for arg in &mut bound.args {
                    *arg = self.ty(*arg);
                }
            }
        }
        for param in &mut signature.params {
            *param = self.ty(*param);
        }
        signature.output = self.ty(signature.output);
        for default in &mut signature.selector_defaults {
            if let Some(crate::EffectArgRef::Type(ty)) = default {
                *ty = self.ty(*ty);
            }
        }
    }

    fn spec_signature(&mut self, signature: &mut SpecSignature) {
        if let Some(callable) = &mut signature.callable {
            self.callable(callable);
        }
        for method in &mut signature.methods {
            if let Some(callable) = &mut method.signature {
                self.callable(callable);
            }
        }
    }

    fn spec_impl(&mut self, implementation: &mut SpecImplFact) {
        implementation.self_type = self.ty(implementation.self_type);
        for arg in &mut implementation.args {
            *arg = self.ty(*arg);
        }
    }

    fn type_spec_satisfaction(&mut self, fact: &mut TypeSpecSatisfactionFact) {
        fact.self_type = self.ty(fact.self_type);
        for arg in &mut fact.args {
            *arg = self.ty(*arg);
        }
    }

    fn callable_spec_satisfaction(&mut self, fact: &mut CallableSpecSatisfactionFact) {
        fact.effect_bound = fact.effect_bound.take().map(|row| self.effect_row(row));
        for arg in &mut fact.args {
            *arg = self.ty(*arg);
        }
    }

    fn trace_spec_conformance(&mut self, fact: &mut TraceSpecConformanceFact) {
        if let TraceSpecConformanceTarget::Named { args, .. } = &mut fact.target {
            for arg in args {
                *arg = self.ty(*arg);
            }
        }
    }

    fn external_callable_spec_satisfaction(
        &mut self,
        fact: &mut ExternalCallableSpecSatisfactionFact,
    ) {
        for arg in &mut fact.args {
            *arg = self.ty(*arg);
        }
    }

    fn external_trace_spec_conformance(&mut self, fact: &mut ExternalTraceSpecConformanceFact) {
        if let ExternalTraceSpecConformanceTarget::Named { args, .. } = &mut fact.target {
            for arg in args {
                *arg = self.ty(*arg);
            }
        }
    }

    fn index_kind(&mut self, kind: &mut CheckedIndexKind) {
        match kind {
            CheckedIndexKind::Sequence {
                base,
                index,
                output,
            } => {
                *base = self.ty(*base);
                *index = self.ty(*index);
                *output = self.ty(*output);
            }
            CheckedIndexKind::MapLookup { key, value } => {
                *key = self.ty(*key);
                *value = self.ty(*value);
            }
        }
    }

    fn slice_kind(&mut self, kind: &mut CheckedSliceKind) {
        match kind {
            CheckedSliceKind::Sequence {
                base,
                start,
                end,
                output,
            } => {
                *base = self.ty(*base);
                *start = self.ty(*start);
                *end = self.ty(*end);
                *output = self.ty(*output);
            }
            CheckedSliceKind::Range {
                range,
                start,
                end,
                output,
            } => {
                *range = self.ty(*range);
                *start = self.ty(*start);
                *end = self.ty(*end);
                *output = self.ty(*output);
            }
        }
    }

    fn try_fact(&mut self, fact: &mut TryExprTypeFact) {
        fact.value_type = self.ty(fact.value_type);
        fact.result_type = self.ty(fact.result_type);
        fact.target_error = fact.target_error.map(|ty| self.ty(ty));
    }

    fn effect_row(&mut self, mut row: EffectRowRef) -> EffectRowRef {
        row.effects = row
            .effects
            .into_iter()
            .map(|effect| self.effect_ref(effect))
            .collect();
        row
    }

    fn effect_ref(&mut self, mut effect: EffectRef) -> EffectRef {
        effect.args = effect
            .args
            .into_iter()
            .map(|arg| self.effect_arg(arg))
            .collect();
        effect
    }

    fn effect_arg(&mut self, arg: EffectArgRef) -> EffectArgRef {
        match arg {
            EffectArgRef::Type(ty) => EffectArgRef::Type(self.ty(ty)),
            EffectArgRef::Wildcard
            | EffectArgRef::String(_)
            | EffectArgRef::Int(_)
            | EffectArgRef::Path(_) => arg,
        }
    }
}

pub(crate) fn substitute_facts(
    facts: &mut crate::TypeFacts,
    target: &mut TypeInterner,
    substitutions: &crate::Substitution,
) -> Result<(), String> {
    let mut remapper = TypeIdRemapper {
        source: None,
        substitutions: Some(substitutions),
        target,
        ids: HashMap::new(),
        base_len: 0,
        error: None,
    };
    remap_facts(facts, &mut remapper);
    match remapper.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(crate) fn project_signature_facts(
    facts: &mut crate::TypeFacts,
    source: &TypeStore,
    target: &mut TypeInterner,
    base_len: usize,
) -> Result<(), String> {
    let mut remapper = TypeIdRemapper::new(source, target);
    remapper.base_len = base_len;
    remap_facts(facts, &mut remapper);
    match remapper.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn remap_facts(facts: &mut crate::TypeFacts, remapper: &mut TypeIdRemapper<'_, '_>) {
    for (_, target) in facts.external_type_aliases.values_mut() {
        *target = remapper.ty(*target);
    }
    for method in facts.type_impl_methods.values_mut() {
        method.self_type = remapper.ty(method.self_type);
    }
    facts.type_generics = std::mem::take(&mut facts.type_generics)
        .into_iter()
        .map(|(ty, mut parameters)| {
            for parameter in &mut parameters {
                parameter.subject = remapper.ty(parameter.subject);
                for bound in &mut parameter.bounds {
                    for argument in &mut bound.args {
                        *argument = remapper.ty(*argument);
                    }
                }
            }
            (remapper.ty(ty), parameters)
        })
        .collect();
    facts.reflection_type_identities = std::mem::take(&mut facts.reflection_type_identities)
        .into_iter()
        .map(|(ty, identity)| (remapper.ty(ty), identity))
        .collect();
    for method in facts.std_intrinsic_calls.values_mut() {
        method.callable_type = remapper.ty(method.callable_type);
        remapper.callable(&mut method.signature);
    }
    for fact in facts.definition_operations.values_mut() {
        fact.contract_type = remapper.ty(fact.contract_type);
    }
    for fact in facts.action_calls.values_mut() {
        remapper.callable(&mut fact.signature);
    }
    for method in facts.agent_methods.values_mut() {
        method.component = remapper.ty(method.component);
        method.callable_type = remapper.ty(method.callable_type);
        remapper.callable(&mut method.callable);
    }
    for fact in facts.handler_actions.values_mut() {
        fact.output = remapper.ty(fact.output);
        for ty in &mut fact.params {
            *ty = remapper.ty(*ty);
        }
        for arg in &mut fact.selectors {
            *arg = remapper.effect_arg(arg.clone());
        }
    }
    for fact in facts.deferred_handler_instantiations.values_mut() {
        if let Some(overload) = &mut fact.overload {
            for ty in overload
                .params
                .iter_mut()
                .chain(std::iter::once(&mut overload.output))
            {
                *ty = remapper.ty(*ty);
            }
            for signature in &mut overload.candidates {
                for ty in signature
                    .params
                    .iter_mut()
                    .chain(std::iter::once(&mut signature.output))
                {
                    *ty = remapper.ty(*ty);
                }
            }
        }
        for parameter in &mut fact.parameters {
            parameter.candidate = remapper.ty(parameter.candidate);
            parameter.rigid = remapper.ty(parameter.rigid);
        }
    }
    for fact in facts.agent_inferences.values_mut() {
        fact.output = remapper.ty(fact.output);
        fact.schema = remapper.ty(fact.schema);
    }
    for fact in facts.reflections.values_mut() {
        fact.descriptor_type = remapper.ty(fact.descriptor_type);
        if let Some(spec) = &mut fact.selected_spec {
            for arg in &mut spec.args {
                *arg = remapper.ty(*arg);
            }
        }
        match &mut fact.target {
            crate::CheckedReflectionTarget::Agent { component, .. }
            | crate::CheckedReflectionTarget::AgentDefaultEntry { component, .. }
            | crate::CheckedReflectionTarget::AgentValue { component, .. } => {
                *component = remapper.ty(*component)
            }
            crate::CheckedReflectionTarget::Callable { .. }
            | crate::CheckedReflectionTarget::ExternalCallable { .. } => {}
            crate::CheckedReflectionTarget::CallableValue { signature, .. } => {
                *signature = remapper.ty(*signature);
            }
        }
    }
    facts.agent_components = std::mem::take(&mut facts.agent_components)
        .into_iter()
        .map(|(ty, symbol)| (remapper.ty(ty), symbol))
        .collect();
    for ty in facts.expr_types.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for ty in facts.expr_memory_places.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for ty in facts.stmt_types.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for ty in facts.pattern_types.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for ty in facts.type_refs.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for fact in facts.symbol_types.values_mut() {
        remapper.symbol_fact(fact);
    }
    for signature in facts.action_signatures.values_mut() {
        remapper.action_signature(signature);
    }
    for signature in facts.qualified_action_signatures.values_mut() {
        remapper.action_signature(signature);
    }
    for fact in facts.generic_instantiations.values_mut() {
        for (_, ty) in &mut fact.type_bindings {
            *ty = remapper.ty(*ty);
        }
        for (_, row) in &mut fact.effect_row_bindings {
            *row = remapper.effect_row(row.clone());
        }
        for obligation in &mut fact.deferred_effect_row_obligations {
            obligation.fixed = remapper.effect_row(obligation.fixed.clone());
            for equation in &mut obligation.concrete {
                equation.fixed = remapper.effect_row(equation.fixed.clone());
                equation.actual = remapper.effect_row(equation.actual.clone());
            }
        }
    }
    facts.known_std_types.directory_list_limit_error = facts
        .known_std_types
        .directory_list_limit_error
        .map(|ty| remapper.ty(ty));
    facts.known_std_types.file_read_limit_error = facts
        .known_std_types
        .file_read_limit_error
        .map(|ty| remapper.ty(ty));
    facts.known_std_types.filesystem_io_error = facts
        .known_std_types
        .filesystem_io_error
        .map(|ty| remapper.ty(ty));
    facts.known_std_types.index_error = facts.known_std_types.index_error.map(|ty| remapper.ty(ty));
    for ty in facts.known_std_types.runtime_types.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for ty in facts.known_std_types.runtime_values.values_mut() {
        *ty = remapper.ty(*ty);
    }
    facts.known_std_types.json_value = facts.known_std_types.json_value.map(|ty| remapper.ty(ty));
    facts.known_std_types.priority_queue = facts
        .known_std_types
        .priority_queue
        .map(|ty| remapper.ty(ty));
    facts.known_std_types.ordered_map = facts.known_std_types.ordered_map.map(|ty| remapper.ty(ty));
    facts.known_std_types.ordered_set = facts.known_std_types.ordered_set.map(|ty| remapper.ty(ty));
    facts.known_std_types.intrinsic_only_constructors =
        std::mem::take(&mut facts.known_std_types.intrinsic_only_constructors)
            .into_iter()
            .map(|ty| remapper.ty(ty))
            .collect();
    facts.known_std_types.projected_fields =
        std::mem::take(&mut facts.known_std_types.projected_fields)
            .into_iter()
            .map(|(ty, fields)| {
                (
                    remapper.ty(ty),
                    fields
                        .into_iter()
                        .map(|field| FieldType {
                            name: field.name,
                            ty: remapper.ty(field.ty),
                        })
                        .collect(),
                )
            })
            .collect();
    facts.known_std_types.opaque_values = std::mem::take(&mut facts.known_std_types.opaque_values)
        .into_iter()
        .map(|ty| remapper.ty(ty))
        .collect();
    let code = &mut facts.known_std_types.code;
    code.agent_definition = code.agent_definition.map(|ty| remapper.ty(ty));
    code.callable_definition = code.callable_definition.map(|ty| remapper.ty(ty));
    code.syntax_block = code.syntax_block.map(|ty| remapper.ty(ty));
    for ty in [
        &mut code.agent_id,
        &mut code.type_info,
        &mut code.generic_owner_info,
        &mut code.tool_ref,
        &mut code.static_value_info,
        &mut code.trace_constraint_info,
        &mut code.definition_id,
        &mut code.callable_id,
        &mut code.effect_row_info,
        &mut code.action_summary,
        &mut code.trace_summary,
        &mut code.determinism,
        &mut code.descriptor_detail,
        &mut code.trace_witness,
    ] {
        *ty = ty.map(|ty| remapper.ty(ty));
    }
    facts.known_std_types.iterables = std::mem::take(&mut facts.known_std_types.iterables)
        .into_iter()
        .map(|(constructor, fact)| (remapper.ty(constructor), fact))
        .collect();
    facts.known_std_types.support_constraints = facts
        .known_std_types
        .support_constraints
        .iter()
        .map(|ty| remapper.ty(*ty))
        .collect();
    facts.enum_layouts = std::mem::take(&mut facts.enum_layouts)
        .into_iter()
        .map(|(ty, mut layout)| {
            for variant in &mut layout.variants {
                variant.fields = variant
                    .fields
                    .iter()
                    .map(|field| remapper.ty(*field))
                    .collect();
            }
            (remapper.ty(ty), layout)
        })
        .collect();
    for fact in facts.resource_handles.values_mut() {
        remapper.resource_handle_fact(fact);
    }
    for signature in facts.item_signatures.values_mut() {
        remapper.item_signature(signature);
    }
    for signature in facts.spec_signatures.values_mut() {
        remapper.spec_signature(signature);
    }
    for signature in facts.external_specs.signatures.values_mut() {
        for method in &mut signature.methods {
            if let Some(callable) = &mut method.signature {
                remapper.callable(callable);
            }
        }
        for param in &mut signature.generic_params {
            param.subject = remapper.ty(param.subject);
            for bound in &mut param.bounds {
                for ty in &mut bound.args {
                    *ty = remapper.ty(*ty);
                }
            }
        }
        for bound in &mut signature.super_specs {
            for ty in &mut bound.args {
                *ty = remapper.ty(*ty);
            }
        }
    }
    for implementation in &mut facts.external_specs.impls {
        implementation.self_type = remapper.ty(implementation.self_type);
        for ty in &mut implementation.args {
            *ty = remapper.ty(*ty);
        }
    }
    for signature in facts.canonical_specs.signatures.values_mut() {
        for bound in signature
            .param_bounds
            .iter_mut()
            .flatten()
            .chain(&mut signature.super_specs)
        {
            for ty in &mut bound.args {
                *ty = remapper.ty(*ty);
            }
        }
    }
    for implementation in &mut facts.canonical_specs.impls {
        implementation.self_type = remapper.ty(implementation.self_type);
        for ty in &mut implementation.args {
            *ty = remapper.ty(*ty);
        }
    }
    for method in &mut facts.external_specs.methods {
        method.self_type = remapper.ty(method.self_type);
        for ty in &mut method.args {
            *ty = remapper.ty(*ty);
        }
        remapper.callable(&mut method.signature);
    }
    for implementation in &mut facts.spec_impls {
        remapper.spec_impl(implementation);
    }
    for implementation in &mut facts.std_spec_impls {
        implementation.self_type = remapper.ty(implementation.self_type);
        for arg in &mut implementation.args {
            *arg = remapper.ty(*arg);
        }
    }
    for fact in &mut facts.type_spec_satisfactions {
        remapper.type_spec_satisfaction(fact);
    }
    for fact in &mut facts.callable_spec_satisfactions {
        remapper.callable_spec_satisfaction(fact);
    }
    for fact in &mut facts.trace_spec_conformances {
        remapper.trace_spec_conformance(fact);
    }
    for fact in &mut facts.external_callable_spec_satisfactions {
        remapper.external_callable_spec_satisfaction(fact);
    }
    for fact in &mut facts.external_trace_spec_conformances {
        remapper.external_trace_spec_conformance(fact);
    }
    for bounds in facts.type_param_bounds.values_mut() {
        for bound in bounds {
            for arg in &mut bound.args {
                *arg = remapper.ty(*arg);
            }
        }
    }
    for kind in facts.index_facts.values_mut() {
        remapper.index_kind(kind);
    }
    for fact in facts.field_projections.values_mut().flatten() {
        fact.receiver = remapper.ty(fact.receiver);
        fact.output = remapper.ty(fact.output);
    }
    for kind in facts.slice_facts.values_mut() {
        remapper.slice_kind(kind);
    }
    for ty in facts.checked_index_errors.values_mut() {
        *ty = remapper.ty(*ty);
    }
    for fact in facts.try_facts.values_mut() {
        remapper.try_fact(fact);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NominalTypeRef, PrimitiveType};
    use etas_code::{DefinitionKey, DefinitionKind, PackageKey};

    fn identity(package: &str) -> DefinitionKey {
        DefinitionKey::new(
            PackageKey::from_name_and_version(package, "1.0.0").unwrap(),
            vec!["app".into()],
            vec!["Token".into()],
            DefinitionKind::Type,
        )
        .unwrap()
    }

    fn nominal(representation: TypeId) -> Type {
        Type::Nominal(NominalTypeRef {
            name: "Token".into(),
            params: Vec::new(),
            representation: Some(representation),
            read_only_erased_view: false,
        })
    }

    #[test]
    fn remap_external_spec_method_types_and_effect_arguments() {
        use crate::facts::specs::{
            ExternalSpecIdentity, ExternalSpecSignatureFact, SpecMethodFact, SpecMethodIdentity,
        };
        let mut source = TypeInterner::new();
        let input = source.primitive(PrimitiveType::I32);
        let output = source.primitive(PrimitiveType::String);
        let source = source.into_store();
        let identity = ExternalSpecIdentity {
            package: 1,
            path: vec!["dep".into(), "Show".into()],
        };
        let mut facts = crate::TypeFacts::default();
        facts.external_specs.signatures.insert(
            identity.clone(),
            ExternalSpecSignatureFact {
                kind: crate::SpecKind::TypeSpec,
                param_names: vec![],
                generic_params: vec![],
                methods: vec![SpecMethodFact {
                    receiver_param: None,
                    identity: SpecMethodIdentity::External {
                        package: 1,
                        path: vec!["dep".into(), "Show".into(), "show".into()],
                    },
                    name: "show".into(),
                    signature: Some(CallableSignature {
                        generic_params: vec![],
                        params: vec![input],
                        output,
                        effects: Some(EffectRowRef {
                            effects: vec![EffectRef {
                                name: "Error".into(),
                                args: vec![EffectArgRef::Type(input)],
                            }],
                            tail: None,
                        }),
                        requested_actions: None,
                    }),
                }],
                super_specs: vec![],
            },
        );
        let mut target = TypeInterner::new();
        target.primitive(PrimitiveType::Bool);
        let mut remapper = TypeIdRemapper::new(&source, &mut target);
        remap_facts(&mut facts, &mut remapper);
        assert!(remapper.error.is_none());
        let signature = facts.external_specs.signatures[&identity].methods[0]
            .signature
            .as_ref()
            .unwrap();
        assert_ne!(signature.params[0], input);
        assert_eq!(
            remapper.target.store().get(signature.params[0]),
            Some(&Type::Primitive(PrimitiveType::I32))
        );
        assert_eq!(
            remapper.target.store().get(signature.output),
            Some(&Type::Primitive(PrimitiveType::String))
        );
        assert_eq!(
            signature.effects.as_ref().unwrap().effects[0].args,
            vec![EffectArgRef::Type(signature.params[0])]
        );
    }

    #[test]
    fn remap_uses_canonical_identity_and_rejects_conflicting_layout() {
        let mut source = TypeInterner::new();
        let boolean = source.primitive(PrimitiveType::Bool);
        let original = source
            .intern_canonical_nominal(identity("first"), nominal(boolean))
            .unwrap();
        let source = source.into_store();

        let mut target = TypeInterner::new();
        let int = target.primitive(PrimitiveType::I32);
        let other = target
            .intern_canonical_nominal(identity("second"), nominal(int))
            .unwrap();
        let mapped = {
            let mut remapper = TypeIdRemapper::new(&source, &mut target);
            let mapped = remapper.ty(original);
            assert!(remapper.error.is_none());
            mapped
        };
        assert_ne!(mapped, other);
        assert_eq!(
            target.store().nominal_identity(mapped),
            Some(&identity("first"))
        );

        let mut conflicting = TypeInterner::new();
        let int = conflicting.primitive(PrimitiveType::I32);
        conflicting
            .intern_canonical_nominal(identity("first"), nominal(int))
            .unwrap();
        let mut remapper = TypeIdRemapper::new(&source, &mut conflicting);
        remapper.ty(original);
        assert_eq!(
            remapper.error.as_deref(),
            Some("canonical nominal identity has conflicting layouts")
        );
    }

    #[test]
    fn remap_preserves_recursive_nominal_identity() {
        let mut source = TypeInterner::new();
        let mut declaration = match nominal(TypeId(0)) {
            Type::Nominal(declaration) => declaration,
            _ => unreachable!(),
        };
        declaration.representation = None;
        let original = source
            .intern_canonical_nominal(identity("recursive"), Type::Nominal(declaration))
            .unwrap();
        let representation = source.intern(Type::Record(crate::RecordType {
            fields: vec![FieldType {
                name: "next".into(),
                ty: original,
            }],
        }));
        source.define_nominal(original, Some(representation));
        let source = source.into_store();
        let mut target = TypeInterner::new();
        let mut remapper = TypeIdRemapper::new(&source, &mut target);
        let mapped = remapper.ty(original);
        assert!(remapper.error.is_none());
        let Some(Type::Nominal(declaration)) = remapper.target.store().get(mapped) else {
            panic!("missing mapped nominal");
        };
        let Some(Type::Record(record)) = declaration
            .representation
            .and_then(|id| remapper.target.store().get(id))
        else {
            panic!("missing mapped representation");
        };
        assert_eq!(record.fields[0].ty, mapped);
    }
}
