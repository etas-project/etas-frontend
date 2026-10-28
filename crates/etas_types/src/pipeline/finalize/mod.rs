mod effect_rows;
mod handler;
pub use handler::{HandlerInstantiationProof, complete_handler_instantiations};
pub mod merge;
pub mod remap;

pub use effect_rows::{
    EffectRowMaterializationError, ResolvedEffectRowInstantiation,
    materialize_effect_row_instantiations,
};
pub use merge::merge_body_deltas as finalize_body_deltas;
