use std::collections::{BTreeMap, HashMap};

mod external_types;

use etas_code::{CallableId, DefinitionKey, DefinitionKind, MethodId, PackageKey};
use etas_hir::{HirItem, HirModuleId, SymbolId};
use etas_std::StdDecl;
use etas_types::{CheckedSpecBound, CheckedSpecRef, SymbolTypeFact, Type, TypeId};

use super::descriptor::ProjectionError;
use crate::CheckedProgramData;

pub fn standard_package_key(
    registry: &etas_std::StdRegistry,
) -> Result<PackageKey, ProjectionError> {
    let contract = &registry.version().contract;
    if contract.is_empty() || contract.contains('\0') {
        return Err(ProjectionError::Invalid(
            "standard definition package has no canonical contract version".into(),
        ));
    }
    Ok(PackageKey::from_canonical_fingerprint(
        etas_code::CodeFingerprint::of_parts(
            "etas.code.builtin-package-origin.v1",
            [b"etas.std".as_slice(), contract.as_bytes()],
        ),
    ))
}

pub fn standard_definition_key(
    registry: &etas_std::StdRegistry,
    package: PackageKey,
    symbol: &etas_std::StdSymbol,
) -> Result<Option<DefinitionKey>, ProjectionError> {
    let kind = match &symbol.decl {
        StdDecl::Type(_) if symbol.kind == etas_std::StdSymbolKind::Constructor => {
            DefinitionKind::Flow
        }
        StdDecl::Type(decl) if decl.kind == etas_std::TypeDeclKind::Spec => DefinitionKind::Spec,
        StdDecl::Type(_) => DefinitionKind::Type,
        StdDecl::Flow(_) if symbol.enum_owner.is_some() => DefinitionKind::EnumVariant,
        StdDecl::Flow(_) => DefinitionKind::Flow,
        StdDecl::Tool(_) => DefinitionKind::Tool,
        StdDecl::Effect(_) => DefinitionKind::Effect,
        StdDecl::Value(_) => DefinitionKind::Value,
        StdDecl::Requirement(_) => DefinitionKind::Requirement,
        _ => return Ok(None),
    };
    let module = registry
        .module(symbol.module)
        .ok_or_else(|| ProjectionError::Invalid("std declaration has no owning module".into()))?;
    let declaration = symbol
        .qualified_path
        .strip_prefix(module.path.as_slice())
        .ok_or_else(|| {
            ProjectionError::Invalid("std declaration path differs from its module".into())
        })?;
    Ok(Some(DefinitionKey::new(
        package,
        module.path.clone(),
        declaration.to_vec(),
        kind,
    )?))
}

/// Package owners supply canonical origin/lock identities. No filesystem lookup
/// or conversion from a package's display name happens during projection.
#[derive(Clone, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DefinitionOrigins {
    pub source_modules: HashMap<HirModuleId, SourceModuleOrigin>,
    pub standard_package: PackageKey,
    pub external_definitions: BTreeMap<Vec<String>, DefinitionKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct SourceModuleOrigin {
    package: PackageKey,
    module: Vec<String>,
}

impl SourceModuleOrigin {
    pub fn new(package: PackageKey, module: Vec<String>) -> Result<Self, ProjectionError> {
        if module
            .iter()
            .any(|segment| segment.is_empty() || segment.contains('\0'))
        {
            return Err(ProjectionError::Invalid(
                "invalid canonical source module origin".into(),
            ));
        }
        Ok(Self { package, module })
    }
    pub fn package(&self) -> PackageKey {
        self.package
    }
    pub fn module(&self) -> &[String] {
        &self.module
    }

    pub fn declared(
        checked: &CheckedProgramData,
        module: HirModuleId,
        package: PackageKey,
    ) -> Result<Self, ProjectionError> {
        let data = checked
            .modules_arena
            .get(module)
            .ok_or(ProjectionError::MissingModule(module))?;
        let source = checked
            .definition_sources
            .source(data.span.source)
            .ok_or_else(|| {
                ProjectionError::Invalid("source module has no immutable source archive".into())
            })?;
        let path = match &source.program.value.module {
            Some(declaration) => declaration
                .path
                .segments
                .iter()
                .map(|segment| segment.text.clone())
                .collect(),
            None if matches!(
                source.kind,
                crate::SourceKind::DependencySourceOverlay { .. }
            ) =>
            {
                return Err(ProjectionError::Invalid(
                    "dependency overlay has no canonical module declaration".into(),
                ));
            }
            None => module_path(checked, module)?,
        };
        Self::new(package, path)
    }
}

impl DefinitionOrigins {
    /// Only the immutable environment that passed the frontend validation gate
    /// may seed portable definition identities.
    pub fn checked_external_definitions(
        checked: &CheckedProgramData,
    ) -> Result<BTreeMap<Vec<String>, DefinitionKey>, ProjectionError> {
        let mut definitions = BTreeMap::new();
        for input in &checked
            .definition_environment
            .environment()
            .external_definition_origins
        {
            for binding in &input.origins.declarations {
                if let Some(previous) =
                    definitions.insert(binding.path(), binding.definition.clone())
                    && previous != binding.definition
                {
                    return Err(ProjectionError::Invalid(
                        "conflicting checked external definition identities".into(),
                    ));
                }
            }
        }
        Ok(definitions)
    }
    /// For an embedding that explicitly owns every declared module in one
    /// package. Dependency overlays must supply their package/module origins.
    pub fn declared_source_modules(
        checked: &CheckedProgramData,
        package: PackageKey,
    ) -> Result<HashMap<HirModuleId, SourceModuleOrigin>, ProjectionError> {
        checked
            .hir
            .modules
            .iter()
            .map(|module| {
                let data = checked
                    .modules_arena
                    .get(*module)
                    .ok_or(ProjectionError::MissingModule(*module))?;
                let origin = checked
                    .module_index
                    .by_source
                    .get(&data.span.source)
                    .and_then(|module| checked.module_index.modules.get(*module))
                    .ok_or_else(|| {
                        ProjectionError::Invalid("checked module has no project origin".into())
                    })?;
                if !matches!(origin.origin, crate::ModuleOrigin::Source) {
                    return Err(ProjectionError::Invalid(
                        "dependency or virtual module requires an explicit package origin".into(),
                    ));
                }
                for part in &origin.parts {
                    let part = checked.module_index.parts.get(*part).ok_or_else(|| {
                        ProjectionError::Invalid("checked module part is missing".into())
                    })?;
                    let source =
                        checked
                            .definition_sources
                            .source(part.source)
                            .ok_or_else(|| {
                                ProjectionError::Invalid(
                                    "module part has no immutable source archive".into(),
                                )
                            })?;
                    if !matches!(
                        source.kind,
                        crate::SourceKind::SourceProjectFile | crate::SourceKind::SingleFileInput
                    ) {
                        return Err(ProjectionError::Invalid(
                            "dependency or virtual module part requires an explicit package origin"
                                .into(),
                        ));
                    }
                }
                Ok((
                    *module,
                    SourceModuleOrigin::declared(checked, *module, package)?,
                ))
            })
            .collect()
    }

    /// Rebind immutable package identities to a rebuilt HIR by exact module
    /// identity. Local arena numbers are never carried across compilations.
    pub(crate) fn rebase_hir(
        &self,
        original: &etas_hir::HirProgram,
        candidate: &etas_hir::HirProgram,
        dependency_modules: &BTreeMap<Vec<String>, SourceModuleOrigin>,
    ) -> Result<Self, ProjectionError> {
        let mut modules = BTreeMap::new();
        for module in &original.modules {
            let path = module_path_in(original, *module)?;
            let origin = self
                .source_modules
                .get(module)
                .cloned()
                .ok_or(ProjectionError::MissingPackage(*module))?;
            if modules.insert(path, origin).is_some() {
                return Err(ProjectionError::Invalid(
                    "frozen environment has duplicate canonical source modules".into(),
                ));
            }
        }
        let mut source_modules = HashMap::new();
        for module in &candidate.modules {
            let path = module_path_in(candidate, *module)?;
            let origin = match modules.remove(&path) {
                Some(origin) => origin,
                None => dependency_modules.get(&path).cloned().ok_or_else(|| {
                    ProjectionError::Invalid(
                        "candidate source module has no locked package origin".into(),
                    )
                })?,
            };
            if source_modules.insert(*module, origin).is_some() {
                return Err(ProjectionError::Invalid(
                    "candidate has duplicate module arena identities".into(),
                ));
            }
        }
        if !modules.is_empty() {
            return Err(ProjectionError::Invalid(
                "candidate removed a frozen source module".into(),
            ));
        }
        Ok(Self {
            source_modules,
            standard_package: self.standard_package,
            external_definitions: self.external_definitions.clone(),
        })
    }

    pub(crate) fn fingerprint(
        &self,
        checked: &CheckedProgramData,
    ) -> Result<etas_code::CodeFingerprint, ProjectionError> {
        use etas_code::CodeFingerprint;
        let mut bindings = vec![self.standard_package.fingerprint()];
        for (module, origin) in self.source_module_bindings(checked)? {
            let package = origin.package.fingerprint();
            let canonical = CodeFingerprint::of_parts(
                "etas.code.canonical-module-origin.v1",
                origin.module.iter().map(|segment| segment.as_bytes()),
            );
            bindings.push(CodeFingerprint::of_parts(
                "etas.code.module-package-origin.v2",
                [
                    package.as_bytes().as_slice(),
                    canonical.as_bytes().as_slice(),
                ]
                .into_iter()
                .chain(module.iter().map(|segment| segment.as_bytes())),
            ));
        }
        for (path, definition) in &self.external_definitions {
            let definition = definition.fingerprint();
            bindings.push(CodeFingerprint::of_parts(
                "etas.code.external-definition-origin.v1",
                std::iter::once(definition.as_bytes().as_slice())
                    .chain(path.iter().map(|segment| segment.as_bytes())),
            ));
        }
        Ok(CodeFingerprint::of_parts(
            "etas.code.definition-origins.v1",
            bindings.iter().map(|binding| binding.as_bytes().as_slice()),
        ))
    }

    pub(crate) fn source_module_bindings(
        &self,
        checked: &CheckedProgramData,
    ) -> Result<BTreeMap<Vec<String>, SourceModuleOrigin>, ProjectionError> {
        let mut modules = BTreeMap::new();
        for module in &checked.hir.modules {
            let path = module_path(checked, *module)?;
            let origin = self
                .source_modules
                .get(module)
                .cloned()
                .ok_or(ProjectionError::MissingPackage(*module))?;
            if modules.insert(path, origin).is_some() {
                return Err(ProjectionError::Invalid(
                    "frozen environment has duplicate canonical source modules".into(),
                ));
            }
        }
        Ok(modules)
    }
}

pub(crate) fn module_path(
    checked: &CheckedProgramData,
    module: HirModuleId,
) -> Result<Vec<String>, ProjectionError> {
    module_path_in(&checked.hir, module)
}

fn module_path_in(
    checked: &etas_hir::HirProgram,
    module: HirModuleId,
) -> Result<Vec<String>, ProjectionError> {
    let module = checked
        .modules_arena
        .get(module)
        .ok_or(ProjectionError::MissingModule(module))?;
    Ok(module
        .name
        .as_ref()
        .map(|name| {
            name.segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Clone, etas_core::RetainedSize)]
pub(crate) struct TypeDeclaration {
    pub identity: DefinitionKey,
    pub parameters: Vec<GenericDeclaration>,
    pub alias: Option<TypeId>,
    pub checked_type: Option<TypeId>,
    pub standard: Option<etas_std::StdSymbolId>,
}

#[derive(Clone, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct GenericDeclaration {
    pub name: String,
    pub effect: bool,
    pub bounds: Vec<BoundDeclaration>,
}

#[derive(Clone, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) enum BoundDeclaration {
    Checked(CheckedSpecBound),
    Standard(etas_std::StdSpecRef),
}

#[derive(etas_core::RetainedSize)]
pub struct DefinitionIdentityIndex {
    standard_package: PackageKey,
    module_packages: HashMap<HirModuleId, PackageKey>,
    symbols: HashMap<SymbolId, DefinitionKey>,
    paths: BTreeMap<Vec<String>, DefinitionKey>,
    methods: HashMap<SymbolId, MethodId>,
    method_paths: BTreeMap<Vec<String>, MethodPath>,
    pub(crate) types: HashMap<TypeId, TypeDeclaration>,
    pub(crate) type_names: HashMap<String, TypeDeclaration>,
    pub(crate) type_declarations: HashMap<SymbolId, TypeDeclaration>,
    effect_tags: HashMap<etas_effects::EffectTagId, DefinitionKey>,
}

#[derive(etas_core::RetainedSize)]
struct MethodPath {
    inherent: bool,
    callable: Option<CallableId>,
}

impl DefinitionIdentityIndex {
    pub fn standard_package(&self) -> PackageKey {
        self.standard_package
    }
    pub fn package_for_module(&self, module: HirModuleId) -> Option<PackageKey> {
        self.module_packages.get(&module).copied()
    }
    pub fn build(
        checked: &CheckedProgramData,
        origins: &DefinitionOrigins,
    ) -> Result<Self, ProjectionError> {
        Self::build_with_limits(checked, origins, Default::default())
    }

    pub fn build_with_limits(
        checked: &CheckedProgramData,
        origins: &DefinitionOrigins,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<Self, ProjectionError> {
        Self::from_input_with_limits(checked.into(), origins, limits)
    }

    pub(crate) fn from_input_with_limits(
        checked: super::descriptor::DescriptorInput<'_>,
        origins: &DefinitionOrigins,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<Self, ProjectionError> {
        let mut canonical_modules = std::collections::HashSet::new();
        for module in &checked.modules {
            let origin = origins
                .source_modules
                .get(module)
                .ok_or(ProjectionError::MissingPackage(*module))?;
            if !canonical_modules.insert((origin.package(), origin.module())) {
                return Err(ProjectionError::Invalid(
                    "multiple source modules claim the same canonical origin".into(),
                ));
            }
        }
        if origins.source_modules.len() != checked.modules.len() {
            return Err(ProjectionError::Invalid(
                "source origin has no owning checked module".into(),
            ));
        }
        let mut index = Self {
            standard_package: origins.standard_package,
            module_packages: origins
                .source_modules
                .iter()
                .map(|(module, origin)| (*module, origin.package()))
                .collect(),
            symbols: HashMap::new(),
            paths: origins.external_definitions.clone(),
            methods: HashMap::new(),
            method_paths: BTreeMap::new(),
            types: HashMap::new(),
            type_names: HashMap::new(),
            type_declarations: HashMap::new(),
            effect_tags: HashMap::new(),
        };
        let mut source_items = HashMap::new();
        let checked_standard_package = standard_package_key(checked.std_registry)?;
        let mut standard_origins = HashMap::new();
        for (item_id, item) in checked.items.iter() {
            let Some(symbol) = item.symbol() else {
                continue;
            };
            // Impl methods receive implementation-owned CallableIds separately.
            if checked.agent_methods.contains_key(&symbol) {
                continue;
            }
            let kind = match item {
                HirItem::Type(_) | HirItem::TypeAlias(_) | HirItem::Enum(_) => DefinitionKind::Type,
                HirItem::Spec(_) => DefinitionKind::Spec,
                HirItem::Flow(_) => DefinitionKind::Flow,
                HirItem::Agent(_) => DefinitionKind::Agent,
                HirItem::Tool(_) => DefinitionKind::Tool,
                HirItem::Effect(_) => DefinitionKind::Effect,
                HirItem::TopLevelLet(_) => DefinitionKind::Value,
                HirItem::Protocol(_) => DefinitionKind::Protocol,
                HirItem::Impl(_) | HirItem::Error { .. } => continue,
            };
            let data = checked
                .symbols
                .get(symbol)
                .ok_or(ProjectionError::MissingSymbol(symbol))?;
            let module = checked
                .modules_arena
                .get(data.defining_module)
                .ok_or(ProjectionError::MissingModule(data.defining_module))?;
            let origin = origins
                .source_modules
                .get(&data.defining_module)
                .ok_or(ProjectionError::MissingPackage(data.defining_module))?;
            let local_path: Vec<String> = module
                .name
                .as_ref()
                .map(|path| {
                    path.segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let key = DefinitionKey::new(
                origin.package(),
                origin.module().to_vec(),
                vec![data.name.clone()],
                kind,
            )?;
            let mut path = local_path;
            path.extend_from_slice(key.declaration());
            index.insert_path(path, key.clone())?;
            if let Some(source) = checked.source_map.item_sources.get(&item_id) {
                source_items.insert(source.span(), key.clone());
            }
            index.symbols.insert(symbol, key);
        }
        for symbol in checked.std_registry.symbols() {
            let Some(key) =
                standard_definition_key(checked.std_registry, origins.standard_package, symbol)?
            else {
                continue;
            };
            let kind = key.kind();
            if let Some(checked_key) =
                standard_definition_key(checked.std_registry, checked_standard_package, symbol)?
            {
                standard_origins.insert(checked_key, key.clone());
            }
            index.insert_path(symbol.qualified_path.clone(), key.clone())?;
            if let StdDecl::Type(decl) = &symbol.decl
                && kind == DefinitionKind::Type
            {
                let parameters = decl
                    .params
                    .iter()
                    .map(|parameter| GenericDeclaration {
                        name: parameter.name.clone(),
                        effect: false,
                        bounds: parameter
                            .bounds
                            .iter()
                            .cloned()
                            .map(BoundDeclaration::Standard)
                            .collect(),
                    })
                    .collect();
                index.type_names.insert(
                    symbol.qualified_path.join("."),
                    TypeDeclaration {
                        identity: key,
                        parameters,
                        alias: None,
                        checked_type: None,
                        standard: Some(symbol.id),
                    },
                );
            }
        }
        // Re-exports keep the registry's canonical declaration identity.
        for module in checked.std_registry.modules() {
            for (name, symbol) in checked.std_registry.module_exports(module.id) {
                if let Some(key) = index.paths.get(&symbol.qualified_path).cloned() {
                    let mut path = module.path.clone();
                    path.push(name.into());
                    index.insert_path(path, key)?;
                }
            }
        }
        for (_, item) in checked.items.iter() {
            let HirItem::Enum(declaration) = item else {
                continue;
            };
            let owner = index
                .symbols
                .get(&declaration.symbol)
                .cloned()
                .ok_or(ProjectionError::MissingDeclaration(declaration.symbol))?;
            let symbol = checked
                .symbols
                .get(declaration.symbol)
                .ok_or(ProjectionError::MissingSymbol(declaration.symbol))?;
            let module = checked
                .modules_arena
                .get(symbol.defining_module)
                .ok_or(ProjectionError::MissingModule(symbol.defining_module))?;
            let local_module = module
                .name
                .as_ref()
                .map(|path| {
                    path.segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for variant in &declaration.variants {
                let symbol = checked
                    .symbols
                    .get(variant.symbol)
                    .ok_or(ProjectionError::MissingSymbol(variant.symbol))?;
                let mut path = owner.declaration().to_vec();
                path.push(symbol.name.clone());
                let identity = DefinitionKey::new(
                    owner.package(),
                    owner.module().to_vec(),
                    path.clone(),
                    DefinitionKind::EnumVariant,
                )?;
                let mut local_path = local_module.clone();
                local_path.extend(path);
                index.insert_path(local_path, identity.clone())?;
                index.symbols.insert(variant.symbol, identity);
            }
        }
        for import in &checked.resolved_imports.imports {
            if import.visibility != etas_hir::Visibility::Public {
                continue;
            }
            let identity = match &import.target {
                crate::ImportTarget::SourceItem { item, .. } => {
                    source_items.get(&item.span).cloned()
                }
                crate::ImportTarget::StdItem { symbol, .. } => checked
                    .std_registry
                    .symbol(*symbol)
                    .and_then(|symbol| index.paths.get(&symbol.qualified_path))
                    .cloned(),
                crate::ImportTarget::ExternalItem {
                    module_path, name, ..
                } => {
                    let mut path = module_path.segments.clone();
                    path.push(name.clone());
                    index.paths.get(&path).cloned()
                }
                crate::ImportTarget::Module(_) => None,
            };
            if let Some(identity) = identity {
                let module = checked
                    .module_index
                    .modules
                    .get(import.from)
                    .ok_or_else(|| {
                        ProjectionError::Invalid("resolved import has no source module".into())
                    })?;
                let mut path = module.path.segments.clone();
                path.push(import.local_name.clone());
                index.insert_path(path, identity)?;
            }
        }
        for import in &checked.resolved_imports.wildcard_imports {
            if import.visibility != etas_hir::Visibility::Public {
                continue;
            }
            let from = checked
                .module_index
                .modules
                .get(import.from)
                .ok_or_else(|| {
                    ProjectionError::Invalid("resolved wildcard import has no source module".into())
                })?;
            for name in &import.exported_names {
                let identity = match &import.target_module {
                    crate::ResolvedModuleTarget::Source { module, .. } => checked
                        .module_index
                        .modules
                        .get(*module)
                        .and_then(|module| module.visibility_exports.items.get(name))
                        .and_then(|export| source_items.get(&export.item.span))
                        .cloned(),
                    crate::ResolvedModuleTarget::Std { path, .. }
                    | crate::ResolvedModuleTarget::External { path, .. } => {
                        let mut path = path.segments.clone();
                        path.push(name.clone());
                        index.paths.get(&path).cloned()
                    }
                };
                if let Some(identity) = identity {
                    let mut path = from.path.segments.clone();
                    path.push(name.clone());
                    index.insert_path(path, identity)?;
                }
            }
        }
        let module_imports = checked
            .resolved_imports
            .imports
            .iter()
            .filter_map(|import| {
                if !matches!(import.target, crate::ImportTarget::Module(_)) {
                    return None;
                }
                let part = checked.module_index.parts.get(import.from_part)?;
                Some((part.source, import.local_name.clone()))
            })
            .collect::<std::collections::HashSet<_>>();
        for symbol in checked.symbols.iter() {
            if let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def {
                if module_imports.contains(&(symbol.definition_span.source, symbol.name.clone())) {
                    continue;
                }
                let key = if let Some(target) = checked.source_import_targets.get(&symbol.id) {
                    Some(
                        index
                            .symbols
                            .get(target)
                            .cloned()
                            .ok_or(ProjectionError::MissingDeclaration(*target))?,
                    )
                } else {
                    checked
                        .std_registry
                        .lookup_qualified(path)
                        .and_then(|symbol| index.paths.get(&symbol.qualified_path))
                        .or_else(|| index.paths.get(path))
                        .cloned()
                };
                if let Some(key) = key {
                    index.symbols.insert(symbol.id, key);
                }
            }
        }
        for (_, item) in checked.items.iter() {
            let Some(symbol) = item.symbol() else {
                continue;
            };
            let Some(identity) = index.symbols.get(&symbol).cloned() else {
                continue;
            };
            let (params, alias): (&[SymbolId], _) = match item {
                HirItem::Type(decl) => (&decl.type_params, None),
                HirItem::Enum(decl) => (&decl.type_params, None),
                HirItem::TypeAlias(decl) => (
                    &decl.type_params,
                    checked.types.type_refs.get(&decl.target).copied(),
                ),
                HirItem::Agent(_) => (&[], None),
                _ => continue,
            };
            let parameters = params
                .iter()
                .map(|param| {
                    let data = checked
                        .symbols
                        .get(*param)
                        .ok_or(ProjectionError::MissingSymbol(*param))?;
                    let checked_bounds = checked.types.type_param_bounds.get(param);
                    if let etas_hir::SymbolDef::TypeParam { bounds, .. } = &data.def
                        && bounds.len() != checked_bounds.map_or(0, Vec::len)
                    {
                        return Err(ProjectionError::Invalid(
                            "declared type binder is missing checked bound facts".into(),
                        ));
                    }
                    Ok(GenericDeclaration {
                        name: data.name.clone(),
                        effect: data.kind == etas_hir::SymbolKind::EffectParam,
                        bounds: checked_bounds
                            .into_iter()
                            .flatten()
                            .map(|bound| {
                                BoundDeclaration::Checked(CheckedSpecBound {
                                    spec: CheckedSpecRef::Source(bound.spec_symbol),
                                    args: bound.args.clone(),
                                })
                            })
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>, ProjectionError>>()?;
            let mut declaration = TypeDeclaration {
                identity,
                parameters,
                alias,
                checked_type: None,
                standard: None,
            };
            let ty = match checked.types.symbol_types.get(&symbol) {
                Some(
                    SymbolTypeFact::Type { constructor }
                    | SymbolTypeFact::NominalType { constructor, .. },
                ) => TypeId(constructor.0),
                Some(SymbolTypeFact::Agent { signature }) => signature.component,
                Some(SymbolTypeFact::TypeAlias { target, .. }) => {
                    // Alias declarations are projected explicitly, not installed as
                    // the identity of their transparent target (e.g. string).
                    if declaration.alias != Some(*target) {
                        return Err(ProjectionError::Invalid(
                            "alias target differs from checked declaration facts".into(),
                        ));
                    }
                    index.type_declarations.insert(symbol, declaration);
                    continue;
                }
                _ => return Err(ProjectionError::MissingDeclaration(symbol)),
            };
            declaration.checked_type = Some(ty);
            index.type_declarations.insert(symbol, declaration.clone());
            index.types.insert(ty, declaration.clone());
            match checked.type_store.get(ty) {
                Some(Type::Nominal(data)) => {
                    if let Some(SymbolTypeFact::NominalType { representation, .. }) =
                        checked.types.symbol_types.get(&symbol)
                        && representation != &data.representation
                    {
                        return Err(ProjectionError::Invalid(
                            "nominal descriptor representation differs from checked declaration facts".into(),
                        ));
                    }
                    index.type_names.insert(data.name.clone(), declaration);
                }
                Some(Type::Enum(data)) => {
                    index.type_names.insert(data.name.clone(), declaration);
                }
                _ => return Err(ProjectionError::MissingType(ty)),
            }
        }
        index.bind_external_types(checked, checked_standard_package)?;
        let declarations = index
            .type_names
            .values()
            .chain(index.type_declarations.values())
            .map(|declaration| (declaration.identity.clone(), declaration.clone()))
            .collect::<HashMap<_, _>>();
        for (ty, identity) in &checked.types.reflection_type_identities {
            if identity.package() != checked_standard_package {
                continue;
            }
            if index.types.contains_key(ty)
                || !matches!(
                    checked.type_store.get(*ty),
                    Some(Type::Named(_) | Type::Nominal(_) | Type::Enum(_))
                )
            {
                continue;
            }
            let identity = standard_origins.get(identity).ok_or_else(|| {
                ProjectionError::MissingIdentity(format!(
                    "checked standard type declaration {identity:?}"
                ))
            })?;
            let mut declaration = declarations.get(identity).cloned().ok_or_else(|| {
                ProjectionError::MissingIdentity(format!("checked type declaration {identity:?}"))
            })?;
            declaration.checked_type = Some(*ty);
            index.types.insert(*ty, declaration);
        }
        for (_, item) in checked.items.iter() {
            if let HirItem::Effect(decl) = item
                && let Some(tag) = checked.effect_registry.tag_by_symbol(decl.symbol)
            {
                let identity = index
                    .symbols
                    .get(&decl.symbol)
                    .ok_or(ProjectionError::MissingDeclaration(decl.symbol))?
                    .clone();
                index.bind_effect(tag, identity)?;
            }
        }
        for symbol in checked.std_registry.symbols() {
            if let StdDecl::Effect(decl) = &symbol.decl
                && let Some(tag) = decl
                    .stable_id
                    .and_then(|id| checked.effect_registry.standard_tag_by_id(id))
            {
                let identity = index
                    .paths
                    .get(&symbol.qualified_path)
                    .ok_or_else(|| {
                        ProjectionError::MissingIdentity(symbol.qualified_path.join("."))
                    })?
                    .clone();
                index.bind_effect(tag, identity)?;
            }
        }
        for tag in checked.effect_registry.tags() {
            if let Some(path) = checked.effect_registry.dependency_tag_path(tag.id)
                && let Some(key) = index.paths.get(path)
            {
                index.bind_effect(tag.id, key.clone())?;
            }
        }
        // Inherent identities do not depend on projected spec arguments. Make
        // them available before projecting implementation headers that use them.
        for (symbol, id) in super::definition::inherent_method_identities(checked, &index, origins)?
        {
            index.bind_method(checked, symbol, id)?;
        }
        for (symbol, id) in
            super::definition::method_identities_from_input(checked, &index, origins, limits)?
        {
            index.bind_method(checked, symbol, id)?;
        }
        Ok(index)
    }

    fn bind_method(
        &mut self,
        checked: super::descriptor::DescriptorInput<'_>,
        symbol: SymbolId,
        id: MethodId,
    ) -> Result<(), ProjectionError> {
        let method = checked
            .types
            .agent_methods
            .get(&symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        let path = checked
            .hir
            .declaration_path(symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        if self
            .methods
            .insert(symbol, id.clone())
            .is_some_and(|previous| previous != id)
        {
            return Err(ProjectionError::Invalid(
                "method has conflicting canonical identities".into(),
            ));
        }
        let callable = CallableId::method(id);
        match self.method_paths.get_mut(&path) {
            Some(previous) if previous.inherent && !method.inherent => {}
            Some(previous) if !previous.inherent && method.inherent => {
                *previous = MethodPath {
                    inherent: true,
                    callable: Some(callable),
                };
            }
            Some(previous) if previous.callable.as_ref() != Some(&callable) => {
                if previous.inherent {
                    return Err(ProjectionError::Invalid(
                        "multiple inherent methods claim one canonical path".into(),
                    ));
                }
                previous.callable = None;
            }
            Some(_) => {}
            None => {
                self.method_paths.insert(
                    path,
                    MethodPath {
                        inherent: method.inherent,
                        callable: Some(callable),
                    },
                );
            }
        }
        Ok(())
    }

    pub(crate) fn method(&self, symbol: SymbolId) -> Option<&MethodId> {
        self.methods.get(&symbol)
    }

    pub(crate) fn callable_path(
        &self,
        path: &[String],
    ) -> Result<Option<&CallableId>, ProjectionError> {
        self.method_paths
            .get(path)
            .map(|path| {
                path.callable.as_ref().ok_or_else(|| {
                    ProjectionError::Invalid(
                        "static callable path has multiple spec-selected implementations".into(),
                    )
                })
            })
            .transpose()
    }

    fn bind_effect(
        &mut self,
        tag: etas_effects::EffectTagId,
        identity: DefinitionKey,
    ) -> Result<(), ProjectionError> {
        if let Some(existing) = self.effect_tags.get(&tag)
            && existing != &identity
        {
            return Err(ProjectionError::Invalid(
                "checked effect tag has conflicting package identities".into(),
            ));
        }
        self.effect_tags.insert(tag, identity);
        Ok(())
    }

    pub(crate) fn effect_target(
        &self,
        checked: &super::descriptor::DescriptorInput<'_>,
        name: &str,
    ) -> Result<etas_code::descriptor::EffectTargetInfo, ProjectionError> {
        if let Some(action) = checked.effect_registry.action_by_name(name) {
            let signature = checked
                .effect_registry
                .action_signature(&action)
                .ok_or_else(|| ProjectionError::MissingIdentity(name.into()))?;
            let effect = self
                .effect_tags
                .get(&signature.owner)
                .ok_or_else(|| ProjectionError::MissingIdentity(name.into()))?
                .clone();
            return Ok(etas_code::descriptor::EffectTargetInfo::Action {
                effect,
                name: signature.name.clone(),
            });
        }
        let key = checked
            .effect_registry
            .tag_by_name(name)
            .and_then(|tag| self.effect_tags.get(&tag))
            .ok_or_else(|| ProjectionError::MissingIdentity(name.into()))?;
        Ok(etas_code::descriptor::EffectTargetInfo::Effect(key.clone()))
    }

    pub(crate) fn effect_tag(
        &self,
        tag: etas_effects::EffectTagId,
    ) -> Result<&DefinitionKey, ProjectionError> {
        self.effect_tags
            .get(&tag)
            .ok_or_else(|| ProjectionError::MissingIdentity(format!("checked effect tag {tag:?}")))
    }

    fn insert_path(
        &mut self,
        path: Vec<String>,
        key: DefinitionKey,
    ) -> Result<(), ProjectionError> {
        if let Some(previous) = self.paths.get(&path)
            && previous != &key
        {
            return Err(ProjectionError::Invalid(format!(
                "conflicting definition identities for {}",
                path.join(".")
            )));
        }
        self.paths.insert(path, key);
        Ok(())
    }

    pub fn symbol(&self, symbol: SymbolId) -> Option<&DefinitionKey> {
        self.symbols.get(&symbol)
    }
    pub fn path(&self, path: &[String]) -> Option<&DefinitionKey> {
        self.paths.get(path)
    }
}
