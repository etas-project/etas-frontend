pub mod binding;
pub mod expr;
pub mod item;
mod namespace_call;
pub mod pattern;
pub mod program;
pub mod stmt;
pub mod ty;

pub use program::{
    HirProjectLowering, HirProjectModule, lower_program, lower_project, lower_source,
};
