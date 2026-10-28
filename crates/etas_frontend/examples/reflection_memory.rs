#[path = "support/allocation_probe.rs"]
mod allocation_probe;

use allocation_probe::{reset_peak, sample};
#[path = "support/reflection_probe.rs"]
mod reflection_probe;
use reflection_probe::prepared;

fn main() {
    let helpers = std::env::args()
        .nth(1)
        .map(|value| value.parse().unwrap())
        .unwrap_or(128);
    let (store, expression) = prepared(helpers);
    let anchor = store.reflect_agent(expression).unwrap();
    for count in [1, 10, 100] {
        reset_peak();
        let before = sample();
        let references = (0..count)
            .map(|_| store.reflect_agent(expression).unwrap())
            .collect::<Vec<_>>();
        let after = sample();
        assert!(
            references
                .iter()
                .all(|reference| reference.revision() == anchor.revision())
        );
        println!(
            "{}",
            serde_json::json!({
                "stage": "reflect",
                "references": count,
                "helpers": helpers,
                "allocated_bytes": after.allocated_bytes - before.allocated_bytes,
                "allocations": after.allocations - before.allocations,
                "live_delta": after.live_bytes.saturating_sub(before.live_bytes),
                "peak_delta": after.peak_live_bytes.saturating_sub(before.live_bytes),
                "retained_definition_estimated_bytes": store.retained_bytes().unwrap(),
            })
        );
        drop(references);
    }
    drop(anchor);
    assert_eq!(store.retained_bytes().unwrap(), 0);
    drop(store);
    println!(
        "{}",
        serde_json::json!({"stage": "store-drop", "live_bytes": sample().live_bytes})
    );
}
