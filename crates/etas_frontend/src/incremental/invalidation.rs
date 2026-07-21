use etas_cache::{ArtifactStore, CacheResult, InvalidationReport, InvalidationSelector};

use super::dirty::DirtyArtifactRoots;

pub(crate) fn invalidate_dirty_roots(
    store: &mut impl ArtifactStore,
    roots: DirtyArtifactRoots,
) -> CacheResult<InvalidationReport> {
    store.invalidate(InvalidationSelector::Roots(roots.into_keys()))
}
