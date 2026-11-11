use super::*;
use etas_code::descriptor::StaticEffectArgInfo;

impl DefinitionIdentityIndex {
    pub(crate) fn resource_path(
        &self,
        checked: &super::super::descriptor::DescriptorInput<'_>,
        path: &[String],
    ) -> Result<StaticEffectArgInfo, ProjectionError> {
        if let Some(key) = self.path(path) {
            return Ok(StaticEffectArgInfo::Resource(key.clone()));
        }
        // A member is not a declaration. Only checked resource roots and their
        // schema fields can introduce a canonical resource projection.
        for (symbol, fact) in &checked.types.resource_handles {
            let Some(root) = self.symbol(*symbol) else {
                continue;
            };
            let Some((prefix, _)) = self.paths.iter().find(|(prefix, key)| {
                *key == root && path.starts_with(prefix) && path.len() > prefix.len()
            }) else {
                continue;
            };
            let etas_types::ResourceHandleFact::MemoryRegion { schema, .. } = fact;
            let mut ty = *schema;
            let fields = &path[prefix.len()..];
            for field in fields {
                if let Some(etas_types::Type::MemoryRegion(schema)) = checked.type_store.get(ty) {
                    ty = *schema;
                }
                let record = etas_types::record_fields_with_applied_params(checked.type_store, ty)
                    .map_err(|error| ProjectionError::Invalid(error.to_string()))?
                    .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))?;
                ty = record
                    .fields
                    .iter()
                    .find(|candidate| candidate.name == *field)
                    .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))?
                    .ty;
            }
            return Ok(StaticEffectArgInfo::ResourceProjection {
                root: root.clone(),
                fields: fields.to_vec(),
            });
        }
        Err(ProjectionError::MissingIdentity(path.join(".")))
    }
}
