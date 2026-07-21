pub mod merge;
pub mod remap;

pub fn run(
    signatures: crate::TypeOutput,
    body_outputs: Vec<crate::TypeOutput>,
) -> crate::TypeOutput {
    remap::remap_output(merge::merge_outputs(signatures, body_outputs))
}
