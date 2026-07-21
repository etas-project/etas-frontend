use etas_cache::ArtifactFingerprint;

pub fn fingerprint_bytes(parts: &[&[u8]]) -> ArtifactFingerprint {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    ArtifactFingerprint::new(*hasher.finalize().as_bytes())
}

pub fn fingerprint_text(parts: &[&str]) -> ArtifactFingerprint {
    let bytes = parts.iter().map(|part| part.as_bytes()).collect::<Vec<_>>();
    fingerprint_bytes(&bytes)
}
