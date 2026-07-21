use std::path::PathBuf;

use etas_cache::{CacheNamespace, DiskArtifactStorePolicy};

use crate::artifact::{DiskCacheAccess, FRONTEND_CACHE_NAMESPACE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontendSessionOptions {
    pub cache: FrontendCacheConfig,
    pub cache_namespace: CacheNamespace,
}

impl FrontendSessionOptions {
    pub fn memory_only() -> Self {
        Self {
            cache: FrontendCacheConfig::MemoryOnly,
            cache_namespace: CacheNamespace::new(FRONTEND_CACHE_NAMESPACE),
        }
    }

    pub fn disk_cache(root: impl Into<PathBuf>) -> Self {
        Self {
            cache: FrontendCacheConfig::Disk {
                root: root.into(),
                access: DiskCacheAccess::read_write(),
                policy: DiskArtifactStorePolicy::default(),
            },
            cache_namespace: CacheNamespace::new(FRONTEND_CACHE_NAMESPACE),
        }
    }

    pub fn with_cache(mut self, cache: FrontendCacheConfig) -> Self {
        self.cache = cache;
        self
    }

    pub fn with_cache_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.cache_namespace = CacheNamespace::new(namespace);
        self
    }

    pub fn with_disk_cache_access(mut self, access: DiskCacheAccess) -> Self {
        if let FrontendCacheConfig::Disk {
            access: current, ..
        } = &mut self.cache
        {
            *current = access;
        }
        self
    }
}

impl Default for FrontendSessionOptions {
    fn default() -> Self {
        Self::memory_only()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontendCacheConfig {
    MemoryOnly,
    Disk {
        root: PathBuf,
        access: DiskCacheAccess,
        policy: DiskArtifactStorePolicy,
    },
}
