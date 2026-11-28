//! Disposable artifacts for one checked caller and its installed SDK. A driver
//! supplies a private, user-owned cache directory separate from project inputs.
use crate::{CheckedProgramData, DiskCacheAccess, StandardLibrary};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug)]
pub struct RunPreparationCache {
    directory: PathBuf,
    key: String,
    owner: Arc<OnceLock<std::sync::Weak<CheckedProgramData>>>,
    access: DiskCacheAccess,
    linked: Arc<OnceLock<String>>,
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
                &library.program.reachability.analysis_items,
            ),
            &mut hash,
            config,
        )
        .ok()?;
        for file in &caller.sources.sources {
            bincode::serde::encode_into_std_write(
                (file.id, &file.path, file.text.as_ref()),
                &mut hash,
                config,
            )
            .ok()?;
        }
        Some(Self {
            directory,
            owner: Default::default(),
            key: hash.finalize().to_hex().to_string(),
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
        self.read(&self.directory.join(format!("{}.link", self.key)))
    }
    pub(crate) fn bind_link(&self, payload: &[u8]) {
        self.linked
            .get_or_init(|| blake3::hash(payload).to_hex().to_string());
    }
    pub(crate) fn write_link(&self, payload: &[u8]) {
        self.write(&self.directory.join(format!("{}.link", self.key)), payload);
    }
    fn plan_path(&self, name: &str) -> Option<PathBuf> {
        Some(
            self.directory
                .join(format!("{}.{}.{name}", self.key, self.linked.get()?)),
        )
    }
    pub fn read_plan(&self, sdk: bool) -> Option<Vec<u8>> {
        self.read(&self.plan_path(if sdk { "std-plan" } else { "entry-plan" })?)
    }
    pub fn write_plan(&self, sdk: bool, bytes: &[u8]) {
        if let Some(path) = self.plan_path(if sdk { "std-plan" } else { "entry-plan" }) {
            self.write(&path, bytes);
        }
    }
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        if !self.access.can_read() {
            return None;
        }
        let mut bytes = fs::read(path).ok()?;
        let length = bytes.len().checked_sub(32)?;
        if blake3::hash(&bytes[..length]).as_bytes() != &bytes[length..] {
            return None;
        }
        bytes.truncate(length);
        Some(bytes)
    }
    fn write(&self, path: &Path, bytes: &[u8]) {
        if !self.can_write() {
            return;
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let Ok(mut file) = fs::File::create_new(&tmp) else {
            return;
        };
        let published = file
            .write_all(bytes)
            .and_then(|_| file.write_all(blake3::hash(bytes).as_bytes()))
            .and_then(|_| fs::rename(&tmp, path));
        if published.is_err() {
            let _ = fs::remove_file(tmp);
        }
    }
}
