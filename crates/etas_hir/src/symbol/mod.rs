mod builder;
pub mod data;
pub mod table;
pub mod visibility;

pub use builder::SymbolBuilder;
pub use data::{
    ImportAliasOrigin, PatternBindingOwner, ResourceKind, Symbol, SymbolData, SymbolDef,
    SymbolKind, SyntheticSymbolReason, TopLevelLetClassification,
};
pub use table::SymbolTable;
pub use visibility::Visibility as HirVisibility;
pub use visibility::Visibility;
