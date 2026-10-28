mod agent_impl;
mod body_overlay;
pub use body_overlay::BodyOverlayError;
pub mod binding;
pub mod expr;
pub mod item;
mod namespace_call;
pub mod pattern;
pub mod program;
mod reflection;
pub mod stmt;
pub mod ty;

pub use program::{
    HirProjectLowering, HirProjectModule, HirProjectNamespace, lower_program, lower_project,
    lower_source,
};
