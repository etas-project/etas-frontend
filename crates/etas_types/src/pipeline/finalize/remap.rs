use std::collections::HashMap;

use crate::{
    CallableSignature, CallableSpecSatisfactionFact, CheckedIndexKind, CheckedSliceKind,
    EffectArgRef, EffectRef, EffectRowRef, ExternalCallableSpecSatisfactionFact,
    ExternalTraceSpecConformanceFact, ExternalTraceSpecConformanceTarget, FieldType,
    HandlerProducedEffects, ItemSignature, ResourceHandleFact, ResourceHandleType, SpecImplFact,
    SpecSignature, SymbolTypeFact, TraceSpecConformanceFact, TraceSpecConformanceTarget,
    TryExprTypeFact, Type, TypeConstructorId, TypeId, TypeInterner, TypeOutput,
    TypeSpecSatisfactionFact, TypeStore,
};

pub fn remap_output(output: TypeOutput) -> TypeOutput {
    output
}

pub fn remap_body_output(mut output: TypeOutput, target: &mut TypeInterner) -> TypeOutput {
    let mut remapper = TypeIdRemapper::new(&output.store, target);
    remap_facts(&mut output.facts, &mut remapper);
    output.store = remapper.target_store().clone();
    output
}

struct TypeIdRemapper<'a, 'b> {
    source: &'a TypeStore,
    target: &'b mut TypeInterner,
    ids: HashMap<TypeId, TypeId>,
}

impl<'a, 'b> TypeIdRemapper<'a, 'b> {
    fn new(source: &'a TypeStore, target: &'b mut TypeInterner) -> Self {
        Self {
            source,
            target,
            ids: HashMap::new(),
        }
    }

    fn target_store(&self) -> &TypeStore {
        self.target.store()
    }

    fn ty(&mut self, ty: TypeId) -> TypeId {
        if let Some(mapped) = self.ids.get(&ty).copied() {
            return mapped;
        }
        let Some(source_ty) = self.source.get(ty).cloned() else {
            return ty;
        };
        let mapped_ty = self.type_data(source_ty);
        let mapped = self.target.intern(mapped_ty);
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
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Var(_)
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
            ItemSignature::Flow(signature)
            | ItemSignature::Agent(signature)
            | ItemSignature::Tool(signature) => self.callable(signature),
            ItemSignature::TopLevelLet(signature) => signature.ty = self.ty(signature.ty),
        }
    }

    fn symbol_fact(&mut self, fact: &mut SymbolTypeFact) {
        match fact {
            SymbolTypeFact::Param { ty }
            | SymbolTypeFact::Local { ty, .. }
            | SymbolTypeFact::Field { ty }
            | SymbolTypeFact::Value { ty }
            | SymbolTypeFact::TopLevelLet { ty, .. }
            | SymbolTypeFact::TypeAlias { target: ty, .. } => *ty = self.ty(*ty),
            SymbolTypeFact::Flow { signature }
            | SymbolTypeFact::Agent { signature }
            | SymbolTypeFact::Tool { signature } => self.callable(signature),
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

    fn resource_handle_fact(&mut self, fact: &mut ResourceHandleFact) {
        match fact {
            ResourceHandleFact::MemoryRegion { schema, .. } => {
                *schema = self.ty(*schema);
            }
        }
    }

    fn action_signature(&mut self, signature: &mut crate::EffectActionSignature) {
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

fn remap_facts(facts: &mut crate::TypeFacts, remapper: &mut TypeIdRemapper<'_, '_>) {
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
    facts.known_std_types.index_error = facts.known_std_types.index_error.map(|ty| remapper.ty(ty));
    for fact in facts.resource_handles.values_mut() {
        remapper.resource_handle_fact(fact);
    }
    for signature in facts.item_signatures.values_mut() {
        remapper.item_signature(signature);
    }
    for signature in facts.spec_signatures.values_mut() {
        remapper.spec_signature(signature);
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
