use std::sync::Arc;

use etas_cache::{
    ArtifactFingerprint, ArtifactMeta, CompressionKind, DiskArtifactBytes, DiskArtifactStore,
    DiskArtifactStoreOptions, PayloadCodec, TypedArtifactStore,
};
use etas_frontend::{
    BODY_UNIT_KIND, FRONTEND_ARTIFACT_SCHEMA_VERSION, FRONTEND_COMPILER_VERSION,
    FrontendArtifactKey, FrontendArtifactKind, FrontendDiskArtifactStore, ProjectRevision,
};
use etas_utils::UnitKey;

struct CacheRoot(std::path::PathBuf);

impl Drop for CacheRoot {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn incompatible_effect_facts_are_rejected_before_body_payload_decoding() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    for (schema, compiler, expected) in [
        (42, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (43, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (44, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (45, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (46, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (47, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (48, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (49, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (50, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (51, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (52, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (53, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (54, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (55, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (56, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (57, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (58, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (59, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (60, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (61, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (62, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (76, FRONTEND_COMPILER_VERSION, "schema version mismatch"),
        (
            FRONTEND_ARTIFACT_SCHEMA_VERSION,
            "obsolete-compiler",
            "compiler version mismatch",
        ),
    ] {
        let root = CacheRoot(std::env::temp_dir().join(format!(
            "etas-effect-cache-schema-{}-{nonce}-{schema}-{compiler}",
            std::process::id()
        )));
        let mut old = DiskArtifactStore::open(
            &root.0,
            DiskArtifactStoreOptions {
                compiler_version: compiler.into(),
                cache_schema_version: schema,
            },
        )
        .unwrap();
        let key = FrontendArtifactKey::unit(
            FrontendArtifactKind::EffectFacts,
            UnitKey::new(BODY_UNIT_KIND, 1),
        )
        .to_cache_key();
        old.put_bytes(DiskArtifactBytes {
            key: key.clone(),
            meta: ArtifactMeta::new(
                ProjectRevision(0),
                ArtifactFingerprint::new([7; 32]),
                compiler,
                schema,
            ),
            codec: PayloadCodec::Bincode2,
            compression: CompressionKind::None,
            // Deliberately invalid: compatibility must be checked before decode.
            payload: vec![0xff],
        })
        .unwrap();
        drop(old);
        let store = FrontendDiskArtifactStore::open(&root.0).unwrap();
        let error = store
            .get::<Arc<etas_effects::BodyEffectOutput>>(&key)
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}
