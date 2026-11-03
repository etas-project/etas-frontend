#[path = "../examples/support/allocation_probe.rs"]
mod allocation_probe;
#[path = "../examples/support/reflection_probe.rs"]
mod reflection_probe;

#[test]
fn repeated_reflection_reuses_the_frozen_environment_without_reprojecting_it() {
    let (store, expression) = reflection_probe::prepared(128);
    let anchor = store.reflect_agent(expression).unwrap();
    for count in [1, 10, 100] {
        allocation_probe::reset_peak();
        let before = allocation_probe::sample();
        let references = (0..count)
            .map(|_| store.reflect_agent(expression).unwrap())
            .collect::<Vec<_>>();
        let after = allocation_probe::sample();
        let allocated = after.allocated_bytes - before.allocated_bytes;
        let handles = (count * size_of::<etas_code::definition::DefinitionRef>()) as u64;
        // Allow small harness bookkeeping, not a new environment per lookup.
        assert!(
            allocated <= handles + 4096,
            "{count} reflection hits allocated {allocated} bytes beyond {handles} bytes of handles"
        );
        assert!(
            references
                .iter()
                .all(|reference| reference.is_same_object(&anchor))
        );
    }
    drop(anchor);
    assert_eq!(store.retained_bytes().unwrap(), 0);
}
