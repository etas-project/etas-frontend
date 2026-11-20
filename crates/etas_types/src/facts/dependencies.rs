//! Collect typed references from solved facts, including compiler-introduced ones.
//! This visits semantic data and never interprets source spelling or syntax.
use crate::TypeId;
use serde::{Serialize, Serializer, ser::*};
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct TypeDependencies(pub BTreeSet<TypeId>, pub BTreeSet<etas_hir::SymbolId>);
impl TypeDependencies {
    pub(crate) fn record(&mut self, value: &impl Serialize) {
        value
            .serialize(&mut *self)
            .expect("checked semantic facts serialize");
    }
}
#[derive(Debug)]
pub struct DependencyError;
impl std::fmt::Display for DependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("semantic dependency visitor")
    }
}
impl std::error::Error for DependencyError {}
impl serde::ser::Error for DependencyError {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}
impl<'a> Serializer for &'a mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(Some(value))
    }
    fn serialize_bool(self, _: bool) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_i8(self, _: i8) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_i16(self, _: i16) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_i32(self, _: i32) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_i64(self, _: i64) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_i128(self, _: i128) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_u8(self, _: u8) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_u16(self, _: u16) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_u64(self, _: u64) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_u128(self, _: u128) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_f32(self, _: f32) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_f64(self, _: f64) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_char(self, _: char) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_str(self, _: &str) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let value = v.serialize(&mut *self)?;
        if matches!(name, "TypeId" | "TypeConstructorId") {
            self.0.insert(TypeId(value.expect("typed arena ID")));
        }
        Ok(None)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        _: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let value = v.serialize(&mut *self)?;
        if name == "CheckedSpecRef" && variant == "Source" {
            self.1
                .insert(etas_hir::SymbolId(value.expect("checked source spec ID")));
        }
        Ok(None)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(self)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(self)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(self)
    }
}
impl SerializeSeq for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}
impl SerializeTuple for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}
impl SerializeTupleStruct for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}
impl SerializeTupleVariant for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}
impl SerializeMap for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}
impl SerializeStruct for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}
impl SerializeStructVariant for &mut TypeDependencies {
    type Ok = Option<u32>;
    type Error = DependencyError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), Self::Error> {
        v.serialize(&mut **self)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(None)
    }
}

/// Typed references retained by a checked semantic object.
pub fn type_dependencies(value: &impl Serialize) -> BTreeSet<TypeId> {
    let mut references = TypeDependencies::default();
    references.record(value);
    references.0
}

/// Type and source-spec references carried by solved semantic facts.
pub fn semantic_type_references(
    value: &impl Serialize,
) -> (BTreeSet<TypeId>, BTreeSet<etas_hir::SymbolId>) {
    let mut references = TypeDependencies::default();
    references.record(value);
    (references.0, references.1)
}
