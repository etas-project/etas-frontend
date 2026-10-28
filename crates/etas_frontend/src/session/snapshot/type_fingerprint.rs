use std::collections::HashMap;

use etas_types::{
    EffectArgRef, EffectRowRef, HandlerProducedEffects, ResourceHandleType, Type,
    TypeConstructorId, TypeId, TypeStore,
};

#[cfg(test)]
mod tests;

pub(super) fn type_part(root: TypeId, store: &TypeStore) -> Result<String, String> {
    let mut graph = Graph::new(store);
    let root = graph.reference(root)?;
    graph.encode(&root)?;
    graph.finish()
}

pub(super) fn effect_row_part(
    row: &Option<EffectRowRef>,
    store: &TypeStore,
) -> Result<String, String> {
    let Some(row) = row else {
        return Ok("effects:none".into());
    };
    let mut graph = Graph::new(store);
    let row = graph.row(row.clone())?;
    graph.encode(&row)?;
    graph.finish()
}

/// Arena IDs are replaced by traversal-local edges. Each reachable type is
/// encoded once, including representations, open rows and recursive back edges.
struct Graph<'a> {
    store: &'a TypeStore,
    ids: HashMap<TypeId, TypeId>,
    pending: Vec<TypeId>,
    hasher: blake3::Hasher,
}

impl<'a> Graph<'a> {
    fn new(store: &'a TypeStore) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"etas.frontend.signature-type-graph.v1");
        Self {
            store,
            ids: HashMap::new(),
            pending: Vec::new(),
            hasher,
        }
    }

    fn reference(&mut self, source: TypeId) -> Result<TypeId, String> {
        if let Some(local) = self.ids.get(&source) {
            return Ok(*local);
        }
        if self.store.get(source).is_none() {
            return Err(format!(
                "signature fingerprint references missing type {}",
                source.0
            ));
        }
        let local = TypeId(
            u32::try_from(self.pending.len())
                .map_err(|_| "signature type graph exceeds u32 identity space")?,
        );
        self.ids.insert(source, local);
        self.pending.push(source);
        Ok(local)
    }

    fn encode(&mut self, value: &impl serde::Serialize) -> Result<(), String> {
        let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())
            .map_err(|error| format!("cannot encode signature type graph: {error}"))?;
        self.hasher.update(&(bytes.len() as u64).to_le_bytes());
        self.hasher.update(&bytes);
        Ok(())
    }

    fn finish(mut self) -> Result<String, String> {
        let mut cursor = 0;
        while let Some(source) = self.pending.get(cursor).copied() {
            let data = self.store.get(source).cloned().ok_or_else(|| {
                format!("signature fingerprint references missing type {}", source.0)
            })?;
            let data = self.ty(data)?;
            self.encode(&data)?;
            cursor += 1;
        }
        Ok(format!("type-graph:{}", self.hasher.finalize().to_hex()))
    }

    fn row(&mut self, mut row: EffectRowRef) -> Result<EffectRowRef, String> {
        row.effects.sort_by(|a, b| a.name.cmp(&b.name));
        for effect in &mut row.effects {
            for arg in &mut effect.args {
                if let EffectArgRef::Type(ty) = arg {
                    *ty = self.reference(*ty)?;
                }
            }
        }
        Ok(row)
    }

    fn ty(&mut self, mut ty: Type) -> Result<Type, String> {
        match &mut ty {
            Type::Array(inner)
            | Type::List(inner)
            | Type::Set(inner)
            | Type::Slice(inner)
            | Type::Option(inner)
            | Type::Schema(inner)
            | Type::Message(inner)
            | Type::MemorySelection(inner)
            | Type::MemoryRegion(inner)
            | Type::Range { index: inner }
            | Type::Trust { inner, .. }
            | Type::Refined { base: inner, .. } => *inner = self.reference(*inner)?,
            Type::Map { key, value }
            | Type::Store { key, value }
            | Type::Result {
                ok: key,
                err: value,
            } => {
                *key = self.reference(*key)?;
                *value = self.reference(*value)?;
            }
            Type::Record(record) => {
                record.fields.sort_by(|a, b| a.name.cmp(&b.name));
                for field in &mut record.fields {
                    field.ty = self.reference(field.ty)?;
                }
            }
            Type::Tuple(items) => {
                for item in items {
                    *item = self.reference(*item)?;
                }
            }
            Type::Function(flow) => {
                for input in &mut flow.input {
                    *input = self.reference(*input)?;
                }
                flow.output = self.reference(flow.output)?;
                flow.effects = flow.effects.take().map(|row| self.row(row)).transpose()?;
            }
            Type::Handler(handler) => {
                handler.handled = self.row(handler.handled.clone())?;
                if let HandlerProducedEffects::Explicit(row) = &mut handler.produced {
                    *row = self.row(row.clone())?;
                }
                handler.result = handler
                    .result
                    .map(|result| self.reference(result))
                    .transpose()?;
            }
            Type::Nominal(nominal) => {
                nominal.representation = nominal
                    .representation
                    .map(|representation| self.reference(representation))
                    .transpose()?;
            }
            Type::Applied { constructor, args } => {
                *constructor = TypeConstructorId(self.reference(TypeId(constructor.0))?.0);
                for arg in args {
                    *arg = self.reference(*arg)?;
                }
            }
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                *schema = self.reference(*schema)?
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                *signature = self.reference(*signature)?
            }
            Type::ResourceHandle(ResourceHandleType::Other { args, .. }) => {
                for arg in args {
                    *arg = self.reference(*arg)?;
                }
            }
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Enum(_)
            | Type::Named(_)
            | Type::Prompt
            | Type::PromptPart
            | Type::MemoryPlace(_) => {}
        }
        Ok(ty)
    }
}
