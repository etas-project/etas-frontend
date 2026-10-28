use super::*;

pub(super) fn charge(
    graph: &etas_package_metadata::CallableValueGraph,
    nodes: &mut usize,
    bytes: &mut u64,
) -> Result<(), EffectPipelineError> {
    let count = etas_package_metadata::validate_callable_value_graph(graph)
        .map_err(|error| invalid(&error.to_string()))?;
    let next_nodes = nodes.saturating_add(count);
    if next_nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
        return Err(EffectPipelineError::AnalysisLimitExceeded {
            resource: "metadata callable nodes",
            limit: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
        });
    }
    let next_bytes =
        bytes.saturating_add(etas_core::memory::MemoryFootprint::of(graph).estimated_bytes());
    if next_bytes > etas_package_metadata::MAX_METADATA_UNCOMPRESSED_SECTION_SIZE as u64 {
        return Err(EffectPipelineError::AnalysisLimitExceeded {
            resource: "retained metadata callable bytes",
            limit: etas_package_metadata::MAX_METADATA_UNCOMPRESSED_SECTION_SIZE,
        });
    }
    *nodes = next_nodes;
    *bytes = next_bytes;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> etas_package_metadata::CallableValueGraph {
        etas_package_metadata::CallableValueGraph {
            sources: vec![CallableValueSource::Parameter(
                etas_package_metadata::CallableValueParameter {
                    scope: CallableValueScope::Root,
                    position: 0,
                },
            )],
            values: vec![],
        }
    }

    #[test]
    fn all_retained_graph_nodes_share_one_budget() {
        let mut nodes = etas_package_metadata::MAX_METADATA_GRAPH_NODES - 1;
        let mut bytes = 0;
        assert!(matches!(
            charge(&graph(), &mut nodes, &mut bytes),
            Err(EffectPipelineError::AnalysisLimitExceeded {
                resource: "metadata callable nodes",
                ..
            })
        ));
        assert_eq!(nodes, etas_package_metadata::MAX_METADATA_GRAPH_NODES - 1);
        assert_eq!(bytes, 0);
    }

    #[test]
    fn retained_bytes_are_checked_before_another_graph_is_copied() {
        let mut nodes = 0;
        let mut bytes = etas_package_metadata::MAX_METADATA_UNCOMPRESSED_SECTION_SIZE as u64;
        assert!(matches!(
            charge(&graph(), &mut nodes, &mut bytes),
            Err(EffectPipelineError::AnalysisLimitExceeded {
                resource: "retained metadata callable bytes",
                ..
            })
        ));
        assert_eq!(nodes, 0);
        assert_eq!(
            bytes,
            etas_package_metadata::MAX_METADATA_UNCOMPRESSED_SECTION_SIZE as u64
        );
    }
}
