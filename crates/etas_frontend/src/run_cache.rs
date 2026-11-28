//! Disposable artifacts for one checked caller and its installed SDK. A driver
//! supplies a private, user-owned cache directory separate from project inputs.
use crate::{CheckedProgramData, DiskCacheAccess, StandardLibrary};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug)]
pub struct RunPreparationCache {
    inputs: blake3::Hash,
    slot: PathBuf,
    owner: Arc<OnceLock<std::sync::Weak<CheckedProgramData>>>,
    access: DiskCacheAccess,
    linked: Arc<OnceLock<blake3::Hash>>,
}

impl RunPreparationCache {
    pub fn new(
        directory: PathBuf,
        build: &str,
        sdk: &str,
        library: &StandardLibrary,
        caller: &CheckedProgramData,
        access: DiskCacheAccess,
    ) -> Option<Self> {
        let mut hash = blake3::Hasher::new_derive_key("etas.checked-run.v1");
        // Local arena indexes in the archive require the same checked ordering.
        let config = bincode::config::standard();
        bincode::serde::encode_into_std_write(
            (
                build,
                sdk,
                &caller.compiler_version,
                &caller.compile_options,
                &caller.project_environment_fingerprint,
                &caller.dependency_metadata_fingerprints,
                &caller.sources.stable_source_keys,
                caller.entry,
                caller.hir.program(),
                &caller.type_store,
                &caller.effect_registry,
                &caller.reachability.analysis_items,
                &library.program.reachability.analysis_items,
            ),
            &mut hash,
            config,
        )
        .ok()?;
        // Replace a previous edit of this entry in place. Three immutable
        // records suffice even when the source or installed toolchain changes.
        let mut slot = blake3::Hasher::new_derive_key("etas.checked-run.slot.v1");
        bincode::serde::encode_into_std_write(
            (caller.entry, &caller.compile_options.optimization_level),
            &mut slot,
            config,
        )
        .ok()?;
        for file in &caller.sources.sources {
            bincode::serde::encode_into_std_write((&file.path, file.id), &mut slot, config).ok()?;
            bincode::serde::encode_into_std_write(
                (file.id, &file.path, file.text.as_ref()),
                &mut hash,
                config,
            )
            .ok()?;
        }
        Some(Self {
            slot: directory.join(slot.finalize().to_hex().as_str()),
            owner: Default::default(),
            inputs: hash.finalize(),
            access,
            linked: Default::default(),
        })
    }

    pub(crate) fn set_owner(&self, owner: &crate::CheckedProgram) {
        self.owner.get_or_init(|| Arc::downgrade(owner));
    }
    pub fn for_owner(owner: &crate::CheckedProgram) -> Option<&Self> {
        let cache = owner.standard_library.as_ref()?.run_cache.as_ref()?;
        (cache.owner.get()?.as_ptr() == Arc::as_ptr(owner) && cache.linked.get().is_some())
            .then_some(cache)
    }
    pub fn can_write(&self) -> bool {
        self.access.can_write()
    }

    pub(crate) fn read_link(&self) -> Option<Vec<u8>> {
        self.read("link", self.inputs)
    }
    pub(crate) fn bind_link(&self, payload: &[u8]) {
        self.linked.get_or_init(|| blake3::hash(payload));
    }
    pub(crate) fn write_link(&self, payload: &[u8]) {
        self.write("link", self.inputs, payload);
    }
    fn plan_key(&self) -> Option<blake3::Hash> {
        Some(blake3::keyed_hash(
            self.inputs.as_bytes(),
            self.linked.get()?.as_bytes(),
        ))
    }
    pub fn read_plan(&self, sdk: bool) -> Option<Vec<u8>> {
        self.read(
            if sdk { "std-plan" } else { "entry-plan" },
            self.plan_key()?,
        )
    }
    pub fn write_plan(&self, sdk: bool, bytes: &[u8]) {
        if let Some(key) = self.plan_key() {
            self.write(if sdk { "std-plan" } else { "entry-plan" }, key, bytes);
        }
    }
    fn read(&self, kind: &str, key: blake3::Hash) -> Option<Vec<u8>> {
        use std::io::Read;
        if !self.access.can_read() {
            return None;
        }
        let mut file = fs::File::open(self.slot.with_extension(kind)).ok()?;
        let mut header = [0; 64];
        file.read_exact(&mut header).ok()?;
        if header[..32] != key.as_bytes()[..] {
            return None;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        (blake3::hash(&bytes).as_bytes()[..] == header[32..]).then_some(bytes)
    }
    fn write(&self, kind: &str, key: blake3::Hash, bytes: &[u8]) {
        if !self.can_write() {
            return;
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = self.slot.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let Ok(mut file) = fs::File::create_new(&tmp) else {
            return;
        };
        let published = file
            .write_all(key.as_bytes())
            .and_then(|_| file.write_all(blake3::hash(bytes).as_bytes()))
            .and_then(|_| file.write_all(bytes))
            .and_then(|_| fs::rename(&tmp, self.slot.with_extension(kind)));
        if published.is_err() {
            let _ = fs::remove_file(tmp);
        }
    }
}
