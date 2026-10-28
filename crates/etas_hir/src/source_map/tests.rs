use super::*;

#[test]
fn generated_source_binding_rejects_conflicts_and_both_directions_of_chains() {
    let mut map = SourceMapBuilder::default();
    assert!(map.bind_lexical_source(SourceId(2), SourceId(1)));
    assert!(map.bind_lexical_source(SourceId(2), SourceId(1)));
    assert!(!map.bind_lexical_source(SourceId(2), SourceId(3)));
    assert!(!map.bind_lexical_source(SourceId(3), SourceId(2)));
    assert!(!map.bind_lexical_source(SourceId(1), SourceId(3)));
    let map = map.finish();
    assert_eq!(map.lexical_source(SourceId(2)), SourceId(1));
    assert_eq!(map.lexical_source(SourceId(3)), SourceId(3));
}
