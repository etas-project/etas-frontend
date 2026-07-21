use etas_cache::{
    ArtifactKey, ArtifactMeta, ArtifactStore, CacheResult, CachedArtifact, InvalidationReport,
    InvalidationSelector, MemoryArtifactStore, TypedArtifactStore,
};

use crate::artifact::{DiskCacheAccess, FrontendDiskArtifactStore};

use super::{FrontendCacheConfig, FrontendSessionOptions};

pub enum FrontendSessionStore {
    Memory(MemoryArtifactStore),
    Disk(Box<FrontendDiskArtifactStore>),
}

impl FrontendSessionStore {
    pub fn from_options(options: &FrontendSessionOptions) -> CacheResult<Self> {
        match &options.cache {
            FrontendCacheConfig::MemoryOnly => Ok(Self::Memory(MemoryArtifactStore::new())),
            FrontendCacheConfig::Disk {
                root,
                access,
                policy,
            } => FrontendDiskArtifactStore::open_with_policy_namespace_access(
                root,
                policy.clone(),
                options.cache_namespace.clone(),
                *access,
            )
            .map(Box::new)
            .map(Self::Disk),
        }
    }
}

pub trait FrontendArtifactStore: TypedArtifactStore {
    fn meta_with_disk_access(
        &self,
        key: &ArtifactKey,
        _access: DiskCacheAccess,
    ) -> CacheResult<Option<ArtifactMeta>> {
        self.meta(key)
    }

    fn get_with_disk_access<T: Clone + 'static>(
        &self,
        key: &ArtifactKey,
        _access: DiskCacheAccess,
    ) -> CacheResult<Option<CachedArtifact<T>>> {
        self.get(key)
    }

    fn put_with_disk_access<T: Clone + 'static>(
        &mut self,
        artifact: CachedArtifact<T>,
        _access: DiskCacheAccess,
    ) -> CacheResult<()> {
        self.put(artifact)
    }
}

impl FrontendArtifactStore for MemoryArtifactStore {}

impl FrontendArtifactStore for FrontendDiskArtifactStore {
    fn meta_with_disk_access(
        &self,
        key: &ArtifactKey,
        access: DiskCacheAccess,
    ) -> CacheResult<Option<ArtifactMeta>> {
        FrontendDiskArtifactStore::meta_with_disk_access(self, key, access)
    }

    fn get_with_disk_access<T: Clone + 'static>(
        &self,
        key: &ArtifactKey,
        access: DiskCacheAccess,
    ) -> CacheResult<Option<CachedArtifact<T>>> {
        FrontendDiskArtifactStore::get_with_disk_access(self, key, access)
    }

    fn put_with_disk_access<T: Clone + 'static>(
        &mut self,
        artifact: CachedArtifact<T>,
        access: DiskCacheAccess,
    ) -> CacheResult<()> {
        FrontendDiskArtifactStore::put_with_disk_access(self, artifact, access)
    }
}

impl FrontendArtifactStore for FrontendSessionStore {
    fn meta_with_disk_access(
        &self,
        key: &ArtifactKey,
        access: DiskCacheAccess,
    ) -> CacheResult<Option<ArtifactMeta>> {
        match self {
            Self::Memory(store) => store.meta_with_disk_access(key, access),
            Self::Disk(store) => store.meta_with_disk_access(key, access),
        }
    }

    fn get_with_disk_access<T: Clone + 'static>(
        &self,
        key: &ArtifactKey,
        access: DiskCacheAccess,
    ) -> CacheResult<Option<CachedArtifact<T>>> {
        match self {
            Self::Memory(store) => store.get_with_disk_access(key, access),
            Self::Disk(store) => store.get_with_disk_access(key, access),
        }
    }

    fn put_with_disk_access<T: Clone + 'static>(
        &mut self,
        artifact: CachedArtifact<T>,
        access: DiskCacheAccess,
    ) -> CacheResult<()> {
        match self {
            Self::Memory(store) => store.put_with_disk_access(artifact, access),
            Self::Disk(store) => store.put_with_disk_access(artifact, access),
        }
    }
}

impl ArtifactStore for FrontendSessionStore {
    fn contains(&self, key: &ArtifactKey) -> CacheResult<bool> {
        match self {
            Self::Memory(store) => store.contains(key),
            Self::Disk(store) => store.contains(key),
        }
    }

    fn meta(&self, key: &ArtifactKey) -> CacheResult<Option<ArtifactMeta>> {
        match self {
            Self::Memory(store) => store.meta(key),
            Self::Disk(store) => store.meta(key),
        }
    }

    fn remove(&mut self, key: &ArtifactKey) -> CacheResult<()> {
        match self {
            Self::Memory(store) => store.remove(key),
            Self::Disk(store) => store.remove(key),
        }
    }

    fn invalidate(&mut self, selector: InvalidationSelector) -> CacheResult<InvalidationReport> {
        match self {
            Self::Memory(store) => store.invalidate(selector),
            Self::Disk(store) => store.invalidate(selector),
        }
    }
}

impl TypedArtifactStore for FrontendSessionStore {
    fn get<T: Clone + 'static>(&self, key: &ArtifactKey) -> CacheResult<Option<CachedArtifact<T>>> {
        match self {
            Self::Memory(store) => store.get(key),
            Self::Disk(store) => store.get(key),
        }
    }

    fn put<T: Clone + 'static>(&mut self, artifact: CachedArtifact<T>) -> CacheResult<()> {
        match self {
            Self::Memory(store) => store.put(artifact),
            Self::Disk(store) => store.put(artifact),
        }
    }
}
