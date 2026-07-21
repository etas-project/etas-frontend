use std::{
    any::{Any, TypeId as RustTypeId},
    cell::RefCell,
    path::Path,
    time::Instant,
};

use etas_cache::{
    ArtifactKey, ArtifactMeta, ArtifactStore, CacheError, CacheNamespace, CacheResult,
    CacheTelemetry, CachedArtifact, CompressionKind, DiskArtifactBytes, DiskArtifactStore,
    DiskArtifactStoreOptions, DiskArtifactStorePolicy, DiskPutStatus, DiskReadOptions,
    InvalidationReport, InvalidationSelector, MemoryArtifactStore, PayloadCodec,
    TypedArtifactStore,
};

use super::{
    DiskCacheAccess, FRONTEND_ARTIFACT_SCHEMA_VERSION, FRONTEND_CACHE_NAMESPACE,
    FrontendArtifactKey, FrontendArtifactKind, FrontendUnitKey, PersistenceClass,
    persistence_class,
};
use crate::BODY_UNIT_KIND;

const FRONTEND_COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct FrontendDiskArtifactStore {
    memory: MemoryArtifactStore,
    disk: RefCell<DiskArtifactStore>,
    compression: CompressionKind,
    telemetry: RefCell<CacheTelemetry>,
    namespace: CacheNamespace,
    access: DiskCacheAccess,
}

impl FrontendDiskArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> CacheResult<Self> {
        Self::open_with_compression(root, CompressionKind::Zstd)
    }

    pub fn open_with_compression(
        root: impl AsRef<Path>,
        compression: CompressionKind,
    ) -> CacheResult<Self> {
        Self::open_with_compression_and_policy(
            root,
            compression,
            DiskArtifactStorePolicy::default(),
        )
    }

    pub fn open_with_policy(
        root: impl AsRef<Path>,
        policy: DiskArtifactStorePolicy,
    ) -> CacheResult<Self> {
        Self::open_with_compression_and_policy(root, CompressionKind::Zstd, policy)
    }

    pub fn open_with_policy_namespace_access(
        root: impl AsRef<Path>,
        policy: DiskArtifactStorePolicy,
        namespace: CacheNamespace,
        access: DiskCacheAccess,
    ) -> CacheResult<Self> {
        Self::open_with_compression_policy_namespace_access(
            root,
            CompressionKind::Zstd,
            policy,
            namespace,
            access,
        )
    }

    pub fn open_with_compression_and_policy(
        root: impl AsRef<Path>,
        compression: CompressionKind,
        policy: DiskArtifactStorePolicy,
    ) -> CacheResult<Self> {
        Self::open_with_compression_policy_namespace_access(
            root,
            compression,
            policy,
            CacheNamespace::new(FRONTEND_CACHE_NAMESPACE),
            DiskCacheAccess::read_write(),
        )
    }

    pub fn open_with_compression_policy_namespace_access(
        root: impl AsRef<Path>,
        compression: CompressionKind,
        policy: DiskArtifactStorePolicy,
        namespace: CacheNamespace,
        access: DiskCacheAccess,
    ) -> CacheResult<Self> {
        let disk = DiskArtifactStore::open_with_policy(
            root,
            DiskArtifactStoreOptions {
                compiler_version: FRONTEND_COMPILER_VERSION.to_owned(),
                cache_schema_version: FRONTEND_ARTIFACT_SCHEMA_VERSION,
            },
            policy,
        )?;
        Ok(Self {
            memory: MemoryArtifactStore::new(),
            disk: RefCell::new(disk),
            compression,
            telemetry: RefCell::new(CacheTelemetry::default()),
            namespace,
            access,
        })
    }

    pub fn telemetry(&self) -> CacheTelemetry {
        let mut telemetry = self.telemetry.borrow().clone();
        telemetry.merge(&self.disk.borrow().telemetry());
        telemetry
    }

    pub(crate) fn meta_with_disk_access(
        &self,
        key: &ArtifactKey,
        access: DiskCacheAccess,
    ) -> CacheResult<Option<ArtifactMeta>> {
        if self.access.can_read() && access.can_read() && disk_class(key).is_some() {
            let physical_key = self.physical_key(key);
            if let Some(meta) = self.disk.borrow().meta(&physical_key)? {
                return Ok(Some(self.logical_meta(meta)));
            }
        }
        self.memory.meta(key)
    }

    pub(crate) fn get_with_disk_access<T: Clone + 'static>(
        &self,
        key: &ArtifactKey,
        access: DiskCacheAccess,
    ) -> CacheResult<Option<CachedArtifact<T>>> {
        if self.access.can_read() && access.can_read() && disk_payload_class(key).is_some() {
            if let Some(artifact) = self.get_persisted(key)? {
                return Ok(Some(artifact));
            }
        }
        self.memory.get(key)
    }

    pub(crate) fn put_with_disk_access<T: Clone + 'static>(
        &mut self,
        artifact: CachedArtifact<T>,
        access: DiskCacheAccess,
    ) -> CacheResult<()> {
        if self.access.can_write() && access.can_write() {
            return self.put(artifact);
        }
        self.memory.put(artifact)
    }
}

impl ArtifactStore for FrontendDiskArtifactStore {
    fn contains(&self, key: &ArtifactKey) -> CacheResult<bool> {
        if self.access.can_read() && disk_class(key).is_some() {
            let physical_key = self.physical_key(key);
            if self.disk.borrow().contains(&physical_key)? {
                return Ok(true);
            }
        }
        self.memory.contains(key)
    }

    fn meta(&self, key: &ArtifactKey) -> CacheResult<Option<ArtifactMeta>> {
        if self.access.can_read() && disk_class(key).is_some() {
            let physical_key = self.physical_key(key);
            if let Some(meta) = self.disk.borrow().meta(&physical_key)? {
                return Ok(Some(self.logical_meta(meta)));
            }
        }
        self.memory.meta(key)
    }

    fn remove(&mut self, key: &ArtifactKey) -> CacheResult<()> {
        self.memory.remove(key)?;
        if self.access.can_write() && disk_class(key).is_some() {
            let physical_key = self.physical_key(key);
            return self.disk.borrow_mut().remove(&physical_key);
        }
        Ok(())
    }

    fn invalidate(&mut self, selector: InvalidationSelector) -> CacheResult<InvalidationReport> {
        let memory = self.memory.invalidate(selector.clone())?;
        if !self.access.can_write() {
            return Ok(memory);
        }
        let physical_selector = self.physical_selector(selector);
        let disk = self.disk.borrow_mut().invalidate(physical_selector)?;
        Ok(merge_reports(memory, self.logical_report(disk)))
    }
}

impl TypedArtifactStore for FrontendDiskArtifactStore {
    fn get<T: Clone + 'static>(&self, key: &ArtifactKey) -> CacheResult<Option<CachedArtifact<T>>> {
        if self.access.can_read() && disk_payload_class(key).is_some() {
            if let Some(artifact) = self.get_persisted(key)? {
                return Ok(Some(artifact));
            }
            return self.memory.get(key);
        }
        self.memory.get(key)
    }

    fn put<T: Clone + 'static>(&mut self, artifact: CachedArtifact<T>) -> CacheResult<()> {
        if !self.access.can_write() {
            return self.memory.put(artifact);
        }
        match disk_class(&artifact.key) {
            Some(PersistenceClass::DiskPayload {
                max_payload_bytes, ..
            }) => {
                let serialize_started = Instant::now();
                if let Some(payload) = encode_persisted(&artifact.key, &artifact.value)? {
                    self.telemetry
                        .borrow_mut()
                        .record_serialize_time(&artifact.key, serialize_started.elapsed());
                    if payload.len() as u64 > max_payload_bytes {
                        self.telemetry
                            .borrow_mut()
                            .record_skipped_write(&artifact.key);
                        return self.memory.put(artifact);
                    }
                    let physical_artifact = self.physical_artifact(artifact.clone());
                    let write = self
                        .disk
                        .borrow_mut()
                        .put_bytes_with_report(DiskArtifactBytes {
                            key: physical_artifact.key,
                            meta: physical_artifact.meta,
                            codec: PayloadCodec::Bincode2,
                            compression: self.compression,
                            payload,
                        });
                    match write {
                        Ok(report) => match report.status {
                            DiskPutStatus::Stored(meta) => {
                                debug_assert!(meta.payload_hash.is_some());
                                return Ok(());
                            }
                            DiskPutStatus::Skipped(_) => return self.memory.put(artifact),
                        },
                        Err(CacheError::Unavailable(_)) => {
                            self.telemetry
                                .borrow_mut()
                                .record_skipped_write(&artifact.key);
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            Some(PersistenceClass::DiskMetadataOnly) => {
                let physical_artifact = self.physical_artifact(artifact.clone());
                let write = self
                    .disk
                    .borrow_mut()
                    .put_metadata(physical_artifact.key, physical_artifact.meta);
                match write {
                    Ok(meta) => {
                        debug_assert!(meta.payload_hash.is_none());
                        return self.memory.put(artifact);
                    }
                    Err(CacheError::Unavailable(_)) => {
                        self.telemetry
                            .borrow_mut()
                            .record_skipped_write(&artifact.key);
                    }
                    Err(error) => return Err(error),
                }
            }
            Some(PersistenceClass::MemoryOnly) | None => {}
        }
        self.memory.put(artifact)
    }
}

impl FrontendDiskArtifactStore {
    fn get_persisted<T: Clone + 'static>(
        &self,
        key: &ArtifactKey,
    ) -> CacheResult<Option<CachedArtifact<T>>> {
        let physical_key = self.physical_key(key);
        let Some(meta) = self.disk.borrow().meta(&physical_key)? else {
            self.telemetry.borrow_mut().record_miss(key);
            return Ok(None);
        };
        let options = DiskReadOptions {
            key: physical_key,
            fingerprint: meta.fingerprint,
            compiler_version: meta.compiler_version.clone(),
            cache_schema_version: meta.cache_schema_version,
        };
        let stored = self.disk.borrow_mut().get_bytes(&options);
        let Some(stored) = (match stored {
            Ok(stored) => stored,
            Err(CacheError::Unavailable(_)) => return Ok(None),
            Err(error) => return Err(error),
        }) else {
            return Ok(None);
        };
        if stored.codec != PayloadCodec::Bincode2 {
            return Err(CacheError::InvalidEnvelope(format!(
                "frontend artifact {} uses unsupported payload codec {:?}",
                stored.key, stored.codec
            )));
        }
        let deserialize_started = Instant::now();
        let logical_key = self.logical_key(&stored.key);
        let Some(value) = decode_persisted::<T>(&logical_key, &stored.payload)? else {
            return Ok(None);
        };
        self.telemetry
            .borrow_mut()
            .record_deserialize_time(&logical_key, deserialize_started.elapsed());
        Ok(Some(CachedArtifact {
            key: logical_key,
            meta: self.logical_meta(stored.meta),
            value,
        }))
    }

    fn physical_artifact<T>(&self, artifact: CachedArtifact<T>) -> CachedArtifact<T> {
        CachedArtifact {
            key: self.physical_key(&artifact.key),
            meta: self.physical_meta(artifact.meta),
            value: artifact.value,
        }
    }

    fn physical_meta(&self, mut meta: ArtifactMeta) -> ArtifactMeta {
        meta.dependencies = meta
            .dependencies
            .iter()
            .map(|dependency| self.physical_key(dependency))
            .collect();
        meta
    }

    fn logical_meta(&self, mut meta: ArtifactMeta) -> ArtifactMeta {
        meta.dependencies = meta
            .dependencies
            .iter()
            .map(|dependency| self.logical_key(dependency))
            .collect();
        meta
    }

    fn physical_key(&self, key: &ArtifactKey) -> ArtifactKey {
        if key.namespace.as_str() != FRONTEND_CACHE_NAMESPACE {
            return key.clone();
        }
        ArtifactKey::new(
            self.namespace.as_str(),
            key.kind.as_str(),
            key.unit.as_str(),
        )
    }

    fn logical_key(&self, key: &ArtifactKey) -> ArtifactKey {
        if key.namespace != self.namespace {
            return key.clone();
        }
        ArtifactKey::new(
            FRONTEND_CACHE_NAMESPACE,
            key.kind.as_str(),
            key.unit.as_str(),
        )
    }

    fn physical_selector(&self, selector: InvalidationSelector) -> InvalidationSelector {
        match selector {
            InvalidationSelector::Exact(key) => {
                InvalidationSelector::Exact(self.physical_key(&key))
            }
            InvalidationSelector::Roots(keys) => {
                InvalidationSelector::Roots(keys.iter().map(|key| self.physical_key(key)).collect())
            }
            InvalidationSelector::Namespace(namespace)
                if namespace.as_str() == FRONTEND_CACHE_NAMESPACE =>
            {
                InvalidationSelector::Namespace(self.namespace.clone())
            }
            InvalidationSelector::Kind { namespace, kind }
                if namespace.as_str() == FRONTEND_CACHE_NAMESPACE =>
            {
                InvalidationSelector::Kind {
                    namespace: self.namespace.clone(),
                    kind,
                }
            }
            selector => selector,
        }
    }

    fn logical_report(&self, report: InvalidationReport) -> InvalidationReport {
        InvalidationReport {
            roots: report
                .roots
                .iter()
                .map(|key| self.logical_key(key))
                .collect(),
            invalidated: report
                .invalidated
                .iter()
                .map(|key| self.logical_key(key))
                .collect(),
        }
    }
}

fn disk_payload_class(key: &ArtifactKey) -> Option<PersistenceClass> {
    disk_class(key)
        .and_then(|class| matches!(class, PersistenceClass::DiskPayload { .. }).then_some(class))
}

fn disk_class(key: &ArtifactKey) -> Option<PersistenceClass> {
    let frontend_key = FrontendArtifactKey::from_cache_key(key)?;
    let class = persistence_class(&frontend_key);
    (class != PersistenceClass::MemoryOnly).then_some(class)
}

fn encode_persisted<T: Clone + 'static>(
    key: &ArtifactKey,
    value: &T,
) -> CacheResult<Option<Vec<u8>>> {
    let Some(frontend_key) = FrontendArtifactKey::from_cache_key(key) else {
        return Ok(None);
    };
    let any = value as &dyn Any;
    match (&frontend_key.kind, &frontend_key.unit) {
        (FrontendArtifactKind::ArtifactManifest, FrontendUnitKey::Project) => {
            let Some(value) = any.downcast_ref::<super::FrontendArtifactManifest>() else {
                return Ok(None);
            };
            encode_bincode(&value.to_persisted()).map(Some)
        }
        (FrontendArtifactKind::BodyArtifactReuseIndex, FrontendUnitKey::Project) => {
            let Some(value) = any.downcast_ref::<super::BodyArtifactReuseIndex>() else {
                return Ok(None);
            };
            encode_bincode(&super::PersistedBodyArtifactReuseIndex::from_frontend(
                value,
            ))
            .map(Some)
        }
        (FrontendArtifactKind::SignatureFacts, FrontendUnitKey::Item(_)) => {
            let Some(value) = any.downcast_ref::<etas_types::ItemSignature>() else {
                return Ok(None);
            };
            encode_bincode(&super::PersistedItemSignature::from_frontend(value)).map(Some)
        }
        (FrontendArtifactKind::TypeFacts, FrontendUnitKey::Unit(unit))
            if unit.kind == BODY_UNIT_KIND =>
        {
            let Some(value) = any.downcast_ref::<etas_types::TypeOutput>() else {
                return Ok(None);
            };
            encode_bincode(value).map(Some)
        }
        (FrontendArtifactKind::EffectFacts, FrontendUnitKey::Unit(unit))
            if unit.kind == BODY_UNIT_KIND =>
        {
            let Some(value) = any.downcast_ref::<etas_effects::EffectOutput>() else {
                return Ok(None);
            };
            encode_bincode(value).map(Some)
        }
        _ => Ok(None),
    }
}

fn decode_persisted<T: Clone + 'static>(
    key: &ArtifactKey,
    payload: &[u8],
) -> CacheResult<Option<T>> {
    let Some(frontend_key) = FrontendArtifactKey::from_cache_key(key) else {
        return Ok(None);
    };
    match (&frontend_key.kind, &frontend_key.unit) {
        (FrontendArtifactKind::ArtifactManifest, FrontendUnitKey::Project)
            if RustTypeId::of::<T>() == RustTypeId::of::<super::FrontendArtifactManifest>() =>
        {
            let value: super::PersistedFrontendArtifactManifest = decode_bincode(payload)?;
            let manifest =
                super::FrontendArtifactManifest::from_persisted(value).ok_or_else(|| {
                    CacheError::InvalidEnvelope(
                        "frontend artifact manifest contains unsupported persisted unit keys"
                            .to_owned(),
                    )
                })?;
            downcast_decoded(manifest).map(Some)
        }
        (FrontendArtifactKind::BodyArtifactReuseIndex, FrontendUnitKey::Project)
            if RustTypeId::of::<T>() == RustTypeId::of::<super::BodyArtifactReuseIndex>() =>
        {
            let value: super::PersistedBodyArtifactReuseIndex = decode_bincode(payload)?;
            let index = value.into_frontend().ok_or_else(|| {
                CacheError::InvalidEnvelope(
                    "body artifact reuse index contains unsupported persisted dependency keys"
                        .to_owned(),
                )
            })?;
            downcast_decoded(index).map(Some)
        }
        (FrontendArtifactKind::SignatureFacts, FrontendUnitKey::Item(_))
            if RustTypeId::of::<T>() == RustTypeId::of::<etas_types::ItemSignature>() =>
        {
            let value: super::PersistedItemSignature = decode_bincode(payload)?;
            downcast_decoded(value.into_frontend()).map(Some)
        }
        (FrontendArtifactKind::TypeFacts, FrontendUnitKey::Unit(unit))
            if unit.kind == BODY_UNIT_KIND
                && RustTypeId::of::<T>() == RustTypeId::of::<etas_types::TypeOutput>() =>
        {
            let value: etas_types::TypeOutput = decode_bincode(payload)?;
            downcast_decoded(value).map(Some)
        }
        (FrontendArtifactKind::EffectFacts, FrontendUnitKey::Unit(unit))
            if unit.kind == BODY_UNIT_KIND
                && RustTypeId::of::<T>() == RustTypeId::of::<etas_effects::EffectOutput>() =>
        {
            let value: etas_effects::EffectOutput = decode_bincode(payload)?;
            downcast_decoded(value).map(Some)
        }
        _ => Ok(None),
    }
}

fn encode_bincode<T: serde::Serialize>(value: &T) -> CacheResult<Vec<u8>> {
    bincode::serde::encode_to_vec(value, bincode::config::standard()).map_err(|error| {
        CacheError::InvalidEnvelope(format!("frontend artifact serialization failed: {error}"))
    })
}

fn decode_bincode<T: serde::de::DeserializeOwned>(payload: &[u8]) -> CacheResult<T> {
    let (value, consumed) = bincode::serde::decode_from_slice(payload, bincode::config::standard())
        .map_err(|error| {
            CacheError::InvalidEnvelope(format!(
                "frontend artifact deserialization failed: {error}"
            ))
        })?;
    if consumed != payload.len() {
        return Err(CacheError::InvalidEnvelope(format!(
            "frontend artifact payload has {} trailing bytes",
            payload.len() - consumed
        )));
    }
    Ok(value)
}

fn downcast_decoded<T: Clone + 'static>(value: impl Any) -> CacheResult<T> {
    let boxed: Box<dyn Any> = Box::new(value);
    boxed.downcast::<T>().map(|value| *value).map_err(|_| {
        CacheError::InvalidEnvelope("frontend artifact decoded to wrong type".to_owned())
    })
}

fn merge_reports(left: InvalidationReport, right: InvalidationReport) -> InvalidationReport {
    InvalidationReport {
        roots: dedupe_keys(left.roots.into_iter().chain(right.roots)),
        invalidated: dedupe_keys(left.invalidated.into_iter().chain(right.invalidated)),
    }
}

fn dedupe_keys(keys: impl IntoIterator<Item = ArtifactKey>) -> Vec<ArtifactKey> {
    let mut seen = std::collections::BTreeSet::new();
    let mut unique = Vec::new();
    for key in keys {
        if seen.insert(key.clone()) {
            unique.push(key);
        }
    }
    unique
}
