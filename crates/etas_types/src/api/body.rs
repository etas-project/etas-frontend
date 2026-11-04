use std::sync::Arc;

use etas_core::Diagnostic;
use etas_hir::{HirItemId, HirProgram};

use crate::ty::{SharedTypeBase, TypeBaseId};
use crate::{
    SpecFacts, Type, TypeFacts, TypeOutput, TypeStore, pipeline::symbols::TypeSymbolIndex,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureBaseExtensionError {
    ForeignBase,
    InvalidForeignDeclaration(crate::TypeId),
    InvalidForeignSpec(&'static str),
    TypeLimit { limit: usize },
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for SignatureBaseExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForeignBase => f.write_str("extended types do not belong to this signature base"),
            Self::InvalidForeignDeclaration(id) => {
                write!(f, "invalid or conflicting foreign declaration {id:?}")
            }
            Self::InvalidForeignSpec(reason) => write!(f, "invalid foreign spec facts: {reason}"),
            Self::TypeLimit { limit } => {
                write!(f, "extended type base exceeds its {limit} type limit")
            }
            Self::Interrupted(interruption) => write!(
                f,
                "extended type base construction interrupted: {}",
                interruption.reason
            ),
        }
    }
}

impl std::error::Error for SignatureBaseExtensionError {}

/// Prepared once per signature revision, shared by all body checks.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SignatureEnvironment {
    pub(crate) types: SharedTypeBase,
    pub(crate) facts: Arc<TypeFacts>,
    pub(crate) specs: Arc<SpecFacts>,
    pub(crate) symbols: Arc<TypeSymbolIndex>,
    pub(crate) diagnostics: Arc<[Diagnostic]>,
}

impl SignatureEnvironment {
    pub fn new(hir: &HirProgram, signatures: &TypeOutput) -> Self {
        Self {
            types: SharedTypeBase::new((*signatures.store).clone()),
            facts: signatures.facts.clone(),
            specs: Arc::new(SpecFacts::from(signatures.facts.as_ref())),
            symbols: Arc::new(TypeSymbolIndex::build(hir)),
            diagnostics: signatures.diagnostics.clone().into(),
        }
    }

    pub fn type_base(&self) -> &SharedTypeBase {
        &self.types
    }

    pub fn owns_signature_facts(&self, facts: &Arc<TypeFacts>) -> bool {
        Arc::ptr_eq(&self.facts, facts)
    }

    pub fn with_method_index(&self, methods: Arc<crate::method::MethodIndex>) -> Self {
        let mut result = self.clone();
        let mut facts = (*self.facts).clone();
        facts.methods = methods;
        result.facts = Arc::new(facts);
        result
    }

    pub fn signature_facts(&self) -> &Arc<TypeFacts> {
        &self.facts
    }

    /// Freeze appended types from this exact base with cooperative cancellation
    /// and a type-count limit. Byte and node budgets remain the caller's responsibility.
    pub fn extend_type_base_controlled(
        &self,
        extended: TypeStore,
        max_types: usize,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<Self, SignatureBaseExtensionError> {
        control
            .checkpoint()
            .map_err(SignatureBaseExtensionError::Interrupted)?;
        if !extended.base().is_some_and(|base| base.ptr_eq(&self.types)) {
            return Err(SignatureBaseExtensionError::ForeignBase);
        }
        if extended.len() > max_types {
            return Err(SignatureBaseExtensionError::TypeLimit { limit: max_types });
        }
        if extended.len() == self.types.len() {
            return Ok(self.clone());
        }
        let types =
            SharedTypeBase::new_controlled(extended, max_types, control).map_err(|error| {
                match error {
                    crate::ty::TypeBaseBuildError::TypeLimit { limit } => {
                        SignatureBaseExtensionError::TypeLimit { limit }
                    }
                    crate::ty::TypeBaseBuildError::Interrupted(interruption) => {
                        SignatureBaseExtensionError::Interrupted(interruption)
                    }
                }
            })?;
        Ok(Self {
            types,
            facts: self.facts.clone(),
            specs: self.specs.clone(),
            symbols: self.symbols.clone(),
            diagnostics: self.diagnostics.clone(),
        })
    }

    pub fn accepts(&self, body: &BodyTypeOutput) -> bool {
        body.base == self.types.identity() && body.base_len == self.types.len()
    }

    pub fn extend_checked_declarations_controlled(
        &self,
        extended: TypeStore,
        generics: std::collections::HashMap<crate::TypeId, Vec<crate::CallableGenericParam>>,
        layouts: std::collections::HashMap<crate::TypeId, crate::EnumLayoutFact>,
        specs: crate::facts::RelocatedSpecFacts,
        max_types: usize,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<Self, SignatureBaseExtensionError> {
        for (id, params) in &generics {
            control
                .checkpoint()
                .map_err(SignatureBaseExtensionError::Interrupted)?;
            let Some(owner) = extended.nominal_identity(*id) else {
                return Err(SignatureBaseExtensionError::InvalidForeignDeclaration(*id));
            };
            if self
                .facts
                .type_generics
                .get(id)
                .is_some_and(|old| old != params)
            {
                return Err(SignatureBaseExtensionError::InvalidForeignDeclaration(*id));
            }
            for (index, param) in params.iter().enumerate() {
                control
                    .checkpoint()
                    .map_err(SignatureBaseExtensionError::Interrupted)?;
                let index = u32::try_from(index)
                    .map_err(|_| SignatureBaseExtensionError::InvalidForeignDeclaration(*id))?;
                let key = crate::ty::CanonicalTypeParameterIdentity::new(owner.clone(), index)
                    .map_err(|_| SignatureBaseExtensionError::InvalidForeignDeclaration(*id))?;
                if extended.parameter_identity(param.subject) != Some(&key) {
                    return Err(SignatureBaseExtensionError::InvalidForeignDeclaration(*id));
                }
            }
        }
        for (id, layout) in &layouts {
            control
                .checkpoint()
                .map_err(SignatureBaseExtensionError::Interrupted)?;
            if extended.nominal_identity(*id).is_none()
                || !matches!(extended.get(*id), Some(Type::Enum(_)))
                || self
                    .facts
                    .enum_layouts
                    .get(id)
                    .is_some_and(|old| old != layout)
            {
                return Err(SignatureBaseExtensionError::InvalidForeignDeclaration(*id));
            }
        }
        specs
            .validate(&self.facts, &extended, Some(control))
            .map_err(|error| match error {
                crate::facts::specs::RelocatedSpecValidationError::Invalid(reason) => {
                    SignatureBaseExtensionError::InvalidForeignSpec(reason)
                }
                crate::facts::specs::RelocatedSpecValidationError::Interrupted(error) => {
                    SignatureBaseExtensionError::Interrupted(error)
                }
            })?;
        let has_specs = !specs.canonical.signatures.is_empty()
            || !specs.canonical.impls.is_empty()
            || !specs.std_impls.is_empty();
        let mut result = self.extend_type_base_controlled(extended, max_types, control)?;
        if !generics.is_empty() || !layouts.is_empty() || has_specs {
            control
                .checkpoint()
                .map_err(SignatureBaseExtensionError::Interrupted)?;
            let mut facts = (*self.facts).clone();
            facts.type_generics.extend(generics);
            facts.enum_layouts.extend(layouts);
            specs
                .merge_into(&mut facts, Some(control))
                .map_err(SignatureBaseExtensionError::Interrupted)?;
            if has_specs {
                result.specs = Arc::new(SpecFacts::from(&facts));
            }
            result.facts = Arc::new(facts);
        }
        Ok(result)
    }
}

/// Only body-owned facts and newly interned types. Type IDs below base_len
/// refer to the identified immutable signature universe, never an embedded copy.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct BodyTypeOutput {
    pub owner: HirItemId,
    pub base: TypeBaseId,
    pub base_len: usize,
    pub types: Vec<Type>,
    pub nominal_identities: std::collections::BTreeMap<crate::TypeId, etas_code::DefinitionKey>,
    pub parameter_identities:
        std::collections::BTreeMap<crate::TypeId, crate::ty::CanonicalTypeParameterIdentity>,
    pub facts: TypeFacts,
    pub diagnostics: Vec<Diagnostic>,
}
