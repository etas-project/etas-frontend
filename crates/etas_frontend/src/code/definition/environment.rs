use crate::{CheckedProgram, code::descriptor::ProjectionError};
use etas_code::{CodeFingerprint, DefinitionKey};
use etas_hir::{HirItemId, HirModuleId, ScopeId};
use std::sync::Arc;

/// The target's native immutable compilation environment. It contains no
/// caller frames, Host context, credentials, or filesystem-backed resolver.
#[derive(etas_core::RetainedSize)]
pub struct FrozenDefinitionEnvironment {
    checked: CheckedProgram,
    origins: Arc<crate::code::identity::DefinitionOrigins>,
    contract: Arc<super::FrozenDefinitionContract>,
    declaration: HirItemId,
    default_body: Option<HirItemId>,
    module: HirModuleId,
    declaration_scope: Option<ScopeId>,
    callable_slots: std::collections::BTreeMap<etas_code::CallableId, crate::AstBodyRef>,
    fingerprint: CodeFingerprint,
    dependency_modules:
        std::collections::BTreeMap<Vec<String>, crate::code::identity::SourceModuleOrigin>,
}

impl FrozenDefinitionEnvironment {
    pub(super) fn new(
        checked: CheckedProgram,
        contract: Arc<super::FrozenDefinitionContract>,
        declaration: HirItemId,
        default_body: Option<HirItemId>,
        origins: Arc<crate::code::identity::DefinitionOrigins>,
        body_revisions: CodeFingerprint,
        callable_slots: std::collections::BTreeMap<etas_code::CallableId, crate::AstBodyRef>,
    ) -> Result<Arc<Self>, ProjectionError> {
        let decl = checked.items.get(declaration).ok_or_else(|| {
            ProjectionError::Invalid("frozen definition declaration is missing".into())
        })?;
        let symbol = decl
            .symbol()
            .and_then(|symbol| checked.symbols.get(symbol))
            .ok_or_else(|| {
                ProjectionError::Invalid("frozen definition has no declaration symbol".into())
            })?;
        let valid_kind = matches!(
            (contract.identity().kind(), decl),
            (
                etas_code::DefinitionKind::Agent,
                etas_hir::HirItem::Agent(_)
            ) | (etas_code::DefinitionKind::Flow, etas_hir::HirItem::Flow(_))
        );
        if !valid_kind {
            return Err(ProjectionError::Invalid(
                "frozen definition identity disagrees with its declaration kind".into(),
            ));
        }
        if checked.definition_environment.fingerprint() != checked.project_environment_fingerprint {
            return Err(ProjectionError::Invalid(
                "checked external environment identity changed before freezing".into(),
            ));
        }
        let mut module = symbol.defining_module;
        let declaration_scope = default_body
            .map(|item| -> Result<ScopeId, ProjectionError> {
                let tree = checked.hir.view();
                let body = tree.item(item).ok_or_else(|| {
                    ProjectionError::Invalid("frozen default entry has no HIR declaration".into())
                })?;
                let scope = body.scope().ok_or_else(|| {
                    ProjectionError::Invalid("frozen default method has no parameter scope".into())
                })?;
                let symbol = body
                    .data()
                    .symbol()
                    .and_then(|symbol| checked.symbols.get(symbol))
                    .ok_or_else(|| {
                        ProjectionError::Invalid(
                            "frozen default method has no declaration symbol".into(),
                        )
                    })?;
                module = symbol.defining_module;
                Ok(scope.id())
            })
            .transpose()?;
        let source_ownership = checked
            .definition_sources
            .source_set()
            .files
            .iter()
            .map(|source| (source.id, source))
            .collect::<std::collections::HashMap<_, _>>();
        let mut source_fingerprints = checked
            .sources
            .sources
            .iter()
            .map(|source| {
                let unit = source_ownership.get(&source.id).ok_or_else(|| {
                    ProjectionError::Invalid("frozen source has no ownership metadata".into())
                })?;
                if source.text() != unit.text.as_ref() {
                    return Err(ProjectionError::Invalid(
                        "frozen source content and ownership metadata disagree".into(),
                    ));
                }
                Ok(source_fingerprint(unit))
            })
            .collect::<Result<Vec<_>, ProjectionError>>()?;
        source_fingerprints.sort_unstable();
        let identity_fingerprint = contract.identity().fingerprint();
        let contract_fingerprint = contract.fingerprint();
        let options = checked.compile_options.canonical_options_fingerprint();
        let standard = origins.standard_package.fingerprint();
        let origin_bindings = origins.fingerprint(&checked)?;
        let artifact_schema = crate::FRONTEND_ARTIFACT_SCHEMA_VERSION.to_le_bytes();
        let mut parts = vec![
            identity_fingerprint.as_bytes().as_slice(),
            standard.as_bytes().as_slice(),
            checked.compiler_version.as_bytes(),
            artifact_schema.as_slice(),
            checked.std_registry.version().contract.as_bytes(),
            checked.definition_environment.fingerprint().as_bytes(),
            options.as_bytes(),
            body_revisions.as_bytes().as_slice(),
            origin_bindings.as_bytes().as_slice(),
            contract_fingerprint.as_bytes().as_slice(),
        ];
        parts.extend(
            source_fingerprints
                .iter()
                .map(|fingerprint| fingerprint.as_bytes().as_slice()),
        );
        let fingerprint =
            CodeFingerprint::of_parts("etas.code.frozen-definition-environment.v2", parts);
        let mut dependency_modules = std::collections::BTreeMap::new();
        for input in checked.external_definition_origins() {
            for module in &input.origins.modules {
                let origin = crate::code::identity::SourceModuleOrigin::new(
                    input.origins.package,
                    module.canonical_path.clone(),
                )?;
                if dependency_modules
                    .insert(module.path.clone(), origin.clone())
                    .is_some_and(|previous| previous != origin)
                {
                    return Err(ProjectionError::Invalid(
                        "locked dependency module has conflicting package origins".into(),
                    ));
                }
            }
        }
        Ok(Arc::new(Self {
            checked,
            origins,
            contract,
            declaration,
            default_body,
            module,
            declaration_scope,
            callable_slots,
            fingerprint,
            dependency_modules,
        }))
    }

    pub fn fingerprint(&self) -> CodeFingerprint {
        self.fingerprint
    }
    pub fn identity(&self) -> &DefinitionKey {
        self.contract.identity()
    }
    pub fn contract(&self) -> &super::FrozenDefinitionContract {
        &self.contract
    }
    pub fn callable_application(&self) -> Option<&crate::CheckedCallableApplication> {
        self.contract
            .applied_behavior()
            .map(|behavior| behavior.application())
    }
    pub(crate) fn contract_arc(&self) -> Arc<super::FrozenDefinitionContract> {
        self.contract.clone()
    }
    pub fn declaration(&self) -> HirItemId {
        self.declaration
    }
    pub fn default_body(&self) -> Option<HirItemId> {
        self.default_body
    }
    pub fn definition_module(&self) -> HirModuleId {
        self.module
    }
    pub fn declaration_scope(&self) -> Option<ScopeId> {
        self.declaration_scope
    }
    pub(crate) fn callable_slots(
        &self,
    ) -> &std::collections::BTreeMap<etas_code::CallableId, crate::AstBodyRef> {
        &self.callable_slots
    }
    pub fn checked(&self) -> &CheckedProgram {
        &self.checked
    }
    pub(crate) fn compilation_fingerprint(&self) -> CodeFingerprint {
        let options = self.checked.compile_options.canonical_options_fingerprint();
        CodeFingerprint::of_parts(
            "etas.code.compiler-configuration.v1",
            [
                self.checked.compiler_version.as_bytes(),
                self.checked.std_registry.version().contract.as_bytes(),
                options.as_bytes(),
                crate::FRONTEND_ARTIFACT_SCHEMA_VERSION
                    .to_le_bytes()
                    .as_slice(),
            ],
        )
    }
    pub(crate) fn dependency_info(
        &self,
    ) -> Result<etas_code::report::DependencyInfo, ProjectionError> {
        use etas_code::report::*;
        let mut modules: Vec<_> = self
            .origins
            .source_module_bindings(&self.checked)?
            .into_values()
            .map(|origin| ModuleOriginInfo {
                module: origin.module().to_vec(),
                package: origin.package(),
            })
            .collect();
        modules.sort_by(|a, b| (&a.module, a.package).cmp(&(&b.module, b.package)));
        modules.dedup();
        let bindings = self
            .origins
            .external_definitions
            .iter()
            .map(|(path, definition)| DefinitionBindingInfo {
                path: path.clone(),
                definition: definition.clone(),
            })
            .collect();
        let mut artifacts = self
            .checked
            .dependency_metadata_fingerprints
            .iter()
            .map(|(import_root, fingerprint)| DependencyArtifactInfo {
                import_root: import_root.clone(),
                fingerprint: fingerprint.clone(),
            })
            .collect::<Vec<_>>();
        artifacts.sort_by(|lhs, rhs| {
            (&lhs.import_root, &lhs.fingerprint).cmp(&(&rhs.import_root, &rhs.fingerprint))
        });
        Ok(DependencyInfo {
            environment: self.fingerprint(),
            lexical_owner: self.identity().clone(),
            standard_package: self.origins.standard_package,
            modules,
            bindings,
            artifacts,
        })
    }
    pub(crate) fn origins_for(
        &self,
        checked: &CheckedProgram,
    ) -> Result<crate::code::identity::DefinitionOrigins, ProjectionError> {
        self.origins_for_hir(&checked.hir)
    }
    pub(crate) fn origins_for_hir(
        &self,
        hir: &etas_hir::HirProgram,
    ) -> Result<crate::code::identity::DefinitionOrigins, ProjectionError> {
        self.origins
            .rebase_hir(&self.checked.hir, hir, &self.dependency_modules)
    }
}

fn source_fingerprint(unit: &crate::SourceFile) -> CodeFingerprint {
    let text = CodeFingerprint::of_bytes("etas.code.source-content.v1", unit.text.as_bytes());
    let fingerprint = |kind: &str| {
        CodeFingerprint::of_parts(
            "etas.code.frozen-source.v1",
            [kind.as_bytes(), text.as_bytes().as_slice()],
        )
    };
    match &unit.kind {
        crate::SourceKind::DependencySourceOverlay {
            package,
            import_root,
        } => CodeFingerprint::of_parts(
            "etas.code.frozen-source.v1",
            [
                b"dependency-overlay".as_slice(),
                package.0.to_le_bytes().as_slice(),
                import_root.as_bytes(),
                text.as_bytes().as_slice(),
            ],
        ),
        crate::SourceKind::SourceProjectFile => fingerprint("project"),
        crate::SourceKind::SingleFileInput => fingerprint("single-file"),
        crate::SourceKind::VirtualStd => fingerprint("standard"),
        crate::SourceKind::VirtualGenerated => fingerprint("generated"),
        crate::SourceKind::LspOverlay => fingerprint("editor-overlay"),
    }
}
