//! Process-independent inputs for the interpreter. The driver validates the
//! project and locked dependency snapshots before giving this revision to run.
use super::{RunPreparationCache, revision::Revision};
use crate::{
    CheckedProgram, CheckedProgramData, DiskCacheAccess, ProjectInput, StandardLibrary,
    StandardLibraryInterface,
    library_hir::{decode_prepared_value, encode_prepared_value},
};
use etas_utils::ProfileHandle;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct RunEntryCache(RunPreparationCache);

#[derive(Serialize, Deserialize)]
struct Entry {
    // Use the published registry wire format, including optional declaration fields.
    registry: Vec<u8>,
    interface: RuntimeInterface,
    caller: Revision,
    diagnostics: Vec<etas_core::Diagnostic>,
}

// Checking-only imported signatures and behavior tables live in the SDK. Keep
// the declaration coordinates needed for linked method proofs in the entry.
#[derive(Serialize, Deserialize)]
struct RuntimeInterface {
    descriptors: Arc<etas_code::descriptor::DefinitionDescriptorSet>,
    origins: etas_code::identity::PackageDefinitionOrigins,
    package: crate::ProjectExternalPackageInput,
    modules: Vec<crate::ProjectExternalModuleInput>,
    // Public metadata has tagged variants with a self-describing wire format.
    metadata: Vec<u8>,
}

impl RuntimeInterface {
    fn capture(interface: &StandardLibraryInterface, checked: &CheckedProgramData) -> Option<Self> {
        let mut descriptors = interface.descriptors.clone();
        if checked
            .standard_library
            .as_ref()
            .is_none_or(|sdk| eligible(&sdk.program))
            && descriptors.headers.as_ref().is_some_and(|headers| {
                headers
                    .bodies
                    .as_ref()
                    .is_none_or(|bodies| bodies.entries.is_empty())
            })
        {
            // Execution catalogs bind callable headers. Snapshot coordinates
            // describe definitions for reflection and dynamic compilation.
            let headers = Arc::make_mut(&mut descriptors).headers.as_mut()?;
            headers.snapshots = None;
            headers.bodies = None;
        }
        Some(Self {
            descriptors,
            origins: interface.origins.clone(),
            package: interface.package.clone(),
            modules: interface.modules.clone(),
            metadata: serde_json::to_vec(&interface.metadata).ok()?,
        })
    }

    fn restore(self) -> Option<Arc<StandardLibraryInterface>> {
        Some(Arc::new(StandardLibraryInterface {
            descriptors: self.descriptors,
            origins: self.origins,
            package: self.package,
            modules: self.modules,
            metadata: serde_json::from_slice(&self.metadata).ok()?,
            prepared_external: None,
            prepared_signatures: None,
            signature_fallback: None,
            validation: Default::default(),
        }))
    }
}

#[derive(Serialize, Deserialize)]
struct LinkedSdk {
    program: Revision,
    owners: crate::project::LinkImage,
    registry: etas_std::StdRegistryImage,
}

pub struct CachedRunEntry {
    pub program: CheckedProgram,
    pub diagnostics: Vec<etas_core::Diagnostic>,
    pub dependencies: BTreeSet<crate::ExternalPackageId>,
}

impl Entry {
    fn decode(bytes: &[u8], profile: &ProfileHandle) -> Option<CachedRunEntry> {
        let owner: Self = decode_prepared_value(bytes).ok()?;
        owner.restore(profile)
    }

    fn restore(self, profile: &ProfileHandle) -> Option<CachedRunEntry> {
        let dependencies = self
            .caller
            .sources
            .iter()
            .filter_map(|source| match &source.kind {
                crate::SourceKind::DependencySourceOverlay { package, .. } => Some(*package),
                _ => None,
            })
            .collect();
        let registry: etas_std::StdRegistryImage = serde_json::from_slice(&self.registry).ok()?;
        let registry = Arc::new(registry.into_registry().ok()?);
        let interface = self.interface.restore()?;
        interface.restore_installed_validation(&registry);
        let mut caller = self.caller.restore(registry, profile, Some(&interface))?;
        caller.standard_library_interface = Some(interface);
        Some(CachedRunEntry {
            program: Arc::new(caller),
            dependencies,
            diagnostics: self.diagnostics,
        })
    }
}

impl LinkedSdk {
    fn decode(
        bytes: &[u8],
        profile: &ProfileHandle,
    ) -> Option<(CheckedProgram, crate::project::LinkImage)> {
        let sdk: Self = decode_prepared_value(bytes).ok()?;
        let registry = Arc::new(sdk.registry.into_registry().ok()?);
        let program = sdk.program.restore(registry, profile, None)?;
        Some((Arc::new(program), sdk.owners))
    }
}

impl RunEntryCache {
    pub fn new(
        directory: PathBuf,
        build: &str,
        sdk: &str,
        sdk_root: &Path,
        input: &ProjectInput,
        lock: &[u8],
        access: DiskCacheAccess,
    ) -> Option<Self> {
        let config = bincode::config::standard();
        let mut slot = blake3::Hasher::new_derive_key("etas.run-entry.slot.v1");
        bincode::serde::encode_into_std_write(
            (
                &input.project_root,
                &input.source_root,
                &input.entry,
                input.options.optimization_level,
                input.sources.iter().map(|s| &s.path).collect::<Vec<_>>(),
            ),
            &mut slot,
            config,
        )
        .ok()?;
        let mut inputs = blake3::Hasher::new_derive_key("etas.run-entry.v2");
        bincode::serde::encode_into_std_write(
            (
                slot.finalize().as_bytes(),
                build,
                sdk,
                sdk_root,
                lock,
                &input.options,
                &input.sources,
                &input.environment,
                &input.environment.external_effect_metadata.source_bindings,
            ),
            &mut inputs,
            config,
        )
        .ok()?;
        Some(Self(RunPreparationCache::from_slot(
            directory.join(slot.finalize().to_hex().as_str()),
            inputs.finalize(),
            access,
        )))
    }

    pub fn stage(
        &self,
        checked: &mut CheckedProgram,
        interface: &StandardLibraryInterface,
        diagnostics: &[etas_core::Diagnostic],
        profile: &ProfileHandle,
    ) -> Option<()> {
        if !self.0.can_write() || !eligible(checked) {
            return None;
        }
        let _span = profile.span("run.store_entry", "driver");
        let caller = Revision::capture(checked)?;
        let sdk = if let Some(library) = &checked.standard_library {
            encode_prepared_value(&LinkedSdk {
                program: Revision::capture(&library.program)?,
                owners: crate::project::LinkImage::capture(
                    checked,
                    &library.program,
                    checked,
                    &library.program,
                )?,
                registry: etas_std::StdRegistryImage::capture(&library.program.std_registry)
                    .ok()?,
            })
            .ok()?
        } else {
            Vec::new()
        };
        let bytes = encode_prepared_value(&Entry {
            registry: serde_json::to_vec(
                &etas_std::StdRegistryImage::capture(&checked.std_registry).ok()?,
            )
            .ok()?,
            interface: RuntimeInterface::capture(interface, checked)?,
            caller,
            diagnostics: diagnostics.to_vec(),
        })
        .ok()?;
        profile.counter("run.entry_bytes", (bytes.len() + sdk.len()) as u64);
        let needs_sdk = checked
            .standard_library
            .as_ref()
            .is_some_and(|lib| !lib.program.reachability.analysis_items.is_empty());
        let pending = PendingEntry {
            caller_revision: bytes,
            sdk_revision: sdk,
            needs_sdk,
            sdk: None,
        };
        self.attach(
            checked,
            EntryPlans::Building(std::sync::Mutex::new(Some(pending))),
        );
        Some(())
    }

    fn attach(&self, checked: &mut CheckedProgram, entry: EntryPlans) {
        let cache = RunPreparationCache {
            entry: Some(Arc::new(entry)),
            owner: Default::default(),
            ..self.0.clone()
        };
        Arc::make_mut(checked).entry_run_cache = Some(cache.clone());
        cache.set_owner(checked);
    }

    pub fn load(&self, profile: &ProfileHandle) -> Option<CachedRunEntry> {
        let _span = profile.span("run.load_entry", "driver");
        let mut bytes = self.0.read("run", self.0.inputs)?;
        let sdk_len = usize::try_from(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?)).ok()?;
        let caller_len =
            usize::try_from(u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?)).ok()?;
        let sdk_end = 16usize.checked_add(sdk_len)?;
        let plans_end = sdk_end.checked_add(caller_len)?;
        let revisions = bytes.get(plans_end..)?;
        let caller_size =
            usize::try_from(u64::from_le_bytes(revisions.get(..8)?.try_into().ok()?)).ok()?;
        let (caller_bytes, sdk_bytes) = revisions.get(8..)?.split_at_checked(caller_size)?;
        let (mut entry, sdk) = {
            let _span = profile.span("run.decode_entry", "driver");
            if sdk_bytes.is_empty() {
                (Entry::decode(caller_bytes, profile)?, None)
            } else {
                // Each revision owns its arena and trace codec. Only the recorded
                // cross-revision bindings need both owners in memory.
                std::thread::scope(|scope| {
                    let library = scope.spawn(|| LinkedSdk::decode(sdk_bytes, profile));
                    let caller = Entry::decode(caller_bytes, profile)?;
                    Some((caller, Some(library.join().expect("SDK revision decoder")?)))
                })?
            }
        };
        if let Some((program, owners)) = sdk {
            let caller = Arc::make_mut(&mut entry.program);
            let interface = caller.standard_library_interface.clone()?;
            let base = Arc::new(StandardLibrary {
                run_cache: None,
                profile: profile.clone(),
                installed_layouts: None,
                program,
                interface,
            });
            caller.standard_library = Some(base.clone());
            let (owner, program) = owners.restore(caller, &base.program)?;
            *caller = owner;
            caller.standard_library = Some(Arc::new(StandardLibrary {
                program,
                ..base.as_ref().clone()
            }));
        }
        bytes.truncate(plans_end);
        self.attach(
            &mut entry.program,
            EntryPlans::Ready {
                bytes: bytes.into_boxed_slice(),
                sdk_end,
            },
        );
        Some(entry)
    }
}

fn eligible(program: &CheckedProgramData) -> bool {
    program.residual.is_empty()
        && program.frozen_agent_call_boundaries.is_empty()
        && !program.requires_definition_catalog()
        && program.types.definition_operations.is_empty()
}

/// The interpreter prepares SDK layouts before it publishes the caller layout.
/// Neither a failed preparation nor an interrupted write can publish a partial entry.
#[derive(Debug)]
pub(super) enum EntryPlans {
    Building(std::sync::Mutex<Option<PendingEntry>>),
    Ready { bytes: Box<[u8]>, sdk_end: usize },
}

#[derive(Debug)]
pub(super) struct PendingEntry {
    caller_revision: Vec<u8>,
    sdk_revision: Vec<u8>,
    needs_sdk: bool,
    sdk: Option<Vec<u8>>,
}

impl EntryPlans {
    pub(super) fn plan(&self, sdk: bool) -> Option<&[u8]> {
        let Self::Ready { bytes, sdk_end } = self else {
            return None;
        };
        if sdk {
            Some(&bytes[16..*sdk_end]).filter(|plan| !plan.is_empty())
        } else {
            Some(&bytes[*sdk_end..])
        }
    }
    pub(super) fn publish_plan(&self, cache: &RunPreparationCache, sdk: bool, bytes: &[u8]) {
        let Self::Building(pending) = self else {
            return;
        };
        let mut guard = pending.lock().expect("entry publication");
        if sdk {
            if let Some(pending) = guard.as_mut() {
                pending.sdk = Some(bytes.to_vec());
            }
            return;
        }
        let Some(pending) = guard.take() else {
            return;
        };
        drop(guard);
        if pending.needs_sdk && pending.sdk.is_none() {
            return;
        }
        let sdk = pending.sdk.as_deref().unwrap_or_default();
        cache.write_parts(
            "run",
            cache.inputs,
            &[
                &(sdk.len() as u64).to_le_bytes(),
                &(bytes.len() as u64).to_le_bytes(),
                sdk,
                bytes,
                &(pending.caller_revision.len() as u64).to_le_bytes(),
                &pending.caller_revision,
                &pending.sdk_revision,
            ],
        );
    }
}

impl etas_core::memory::RetainedSize for EntryPlans {
    fn visit_retained<'a>(&'a self, visitor: &mut etas_core::memory::MemoryVisitor<'a>) {
        if let Self::Ready { bytes, .. } = self {
            bytes.visit_retained(visitor);
        }
    }
}
