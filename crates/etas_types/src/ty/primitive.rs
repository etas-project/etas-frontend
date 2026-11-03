#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum PrimitiveType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    ISize,
    U8,
    U16,
    U32,
    U64,
    U128,
    USize,
    F32,
    F64,
    Char,
    String,
    Bytes,
    Unit,
    Never,
}

impl PrimitiveType {
    pub fn from_hir(kind: etas_hir::HirPrimitiveType) -> Self {
        use etas_hir::HirPrimitiveType as H;
        match kind {
            H::Bool => Self::Bool,
            H::I8 => Self::I8,
            H::I16 => Self::I16,
            H::I32 => Self::I32,
            H::I64 => Self::I64,
            H::I128 => Self::I128,
            H::Isize => Self::ISize,
            H::U8 => Self::U8,
            H::U16 => Self::U16,
            H::U32 => Self::U32,
            H::U64 => Self::U64,
            H::U128 => Self::U128,
            H::Usize => Self::USize,
            H::F32 => Self::F32,
            H::F64 => Self::F64,
            H::Char => Self::Char,
            H::String => Self::String,
            H::Bytes => Self::Bytes,
            H::Unit => Self::Unit,
            H::Never => Self::Never,
        }
    }

    pub fn descriptor_info(self) -> etas_code::descriptor::PrimitiveInfo {
        use etas_code::descriptor::PrimitiveInfo as P;
        match self {
            Self::Bool => P::Bool,
            Self::I8 => P::I8,
            Self::I16 => P::I16,
            Self::I32 => P::I32,
            Self::I64 => P::I64,
            Self::I128 => P::I128,
            Self::ISize => P::Isize,
            Self::U8 => P::U8,
            Self::U16 => P::U16,
            Self::U32 => P::U32,
            Self::U64 => P::U64,
            Self::U128 => P::U128,
            Self::USize => P::Usize,
            Self::F32 => P::F32,
            Self::F64 => P::F64,
            Self::Char => P::Char,
            Self::String => P::String,
            Self::Bytes => P::Bytes,
            Self::Unit => P::Unit,
            Self::Never => P::Never,
        }
    }

    pub fn descriptor_fingerprint(
        self,
    ) -> Result<etas_code::CodeFingerprint, etas_code::descriptor::DescriptorError> {
        use etas_code::descriptor::{
            DescriptorGraph, DescriptorGraphInput, DescriptorLimits, TypeInfo, TypeInfoRef,
        };
        let input = DescriptorGraphInput {
            types: vec![TypeInfo::Primitive(self.descriptor_info())],
            ..Default::default()
        };
        DescriptorGraph::new(input, DescriptorLimits::default())?.type_fingerprint(TypeInfoRef(0))
    }

    pub fn source_name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::ISize => "isize",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::USize => "usize",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Char => "char",
            Self::String => "string",
            Self::Bytes => "bytes",
            Self::Unit => "unit",
            Self::Never => "never",
        }
    }
}
