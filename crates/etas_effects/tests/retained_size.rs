use etas_core::memory::MemoryFootprint;
use etas_effects::EffectSummary;

#[test]
fn default_effect_summaries_do_not_claim_heap_for_empty_collections() {
    let summary = EffectSummary::default();
    assert_eq!(MemoryFootprint::of(&summary).estimated_bytes(), 0);

    let summaries = vec![summary; 8_192];
    assert_eq!(
        MemoryFootprint::of(&summaries).estimated_bytes(),
        (summaries.capacity() * size_of::<EffectSummary>()) as u64
    );
}
