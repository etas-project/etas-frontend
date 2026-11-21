//! HIR arenas support linked subsets without changing checked node identities.
use etas_core::Idx;
use std::{marker::PhantomData, ops::Index};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct Arena<Id, T> {
    values: Vec<T>,
    ids: Option<Vec<u32>>,
    extent: usize,
    marker: PhantomData<fn() -> Id>,
}
impl<Id, T> Default for Arena<Id, T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            ids: None,
            extent: 0,
            marker: PhantomData,
        }
    }
}
impl<Id: Idx, T> Arena<Id, T> {
    pub fn alloc(&mut self, value: T) -> Id {
        let id = self.len() as u32;
        if let Some(ids) = &mut self.ids {
            ids.push(id);
            self.extent += 1;
        }
        self.values.push(value);
        Id::from_u32(id)
    }
    pub fn alloc_with_id(&mut self, make: impl FnOnce(Id) -> T) -> Id {
        let id = Id::from_u32(self.len() as u32);
        self.alloc(make(id))
    }
    pub fn iter(&self) -> impl Iterator<Item = (Id, &T)> {
        self.values.iter().enumerate().map(|(index, value)| {
            (
                Id::from_u32(self.ids.as_ref().map_or(index as u32, |ids| ids[index])),
                value,
            )
        })
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Id, &mut T)> {
        let ids = &self.ids;
        self.values
            .iter_mut()
            .enumerate()
            .map(move |(index, value)| {
                (
                    Id::from_u32(ids.as_ref().map_or(index as u32, |ids| ids[index])),
                    value,
                )
            })
    }
    fn position(&self, id: Id) -> Option<usize> {
        match &self.ids {
            Some(ids) => ids.binary_search(&(id.into_usize() as u32)).ok(),
            None => Some(id.into_usize()),
        }
    }
    pub fn get(&self, id: Id) -> Option<&T> {
        self.values.get(self.position(id)?)
    }
    pub fn get_mut(&mut self, id: Id) -> Option<&mut T> {
        let index = self.position(id)?;
        self.values.get_mut(index)
    }
    /// Link disjoint checked objects while preserving their original IDs.
    pub fn extend_linked(&mut self, values: impl IntoIterator<Item = (Id, T)>) {
        let mut extent = self.len();
        let old_ids = self.ids.take();
        let mut entries = std::mem::take(&mut self.values)
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                (
                    old_ids.as_ref().map_or(index as u32, |ids| ids[index]),
                    value,
                )
            })
            .collect::<Vec<_>>();
        entries.extend(
            values
                .into_iter()
                .map(|(id, value)| (id.into_usize() as u32, value)),
        );
        entries.sort_unstable_by_key(|(id, _)| *id);
        if let Some((last, _)) = entries.last() {
            extent = extent.max(*last as usize + 1);
        }
        let (ids, values) = entries.into_iter().unzip();
        self.ids = Some(ids);
        self.values = values;
        self.extent = extent;
    }

    /// The ID address space remains stable when linked nodes are removed.
    pub fn len(&self) -> usize {
        if self.ids.is_some() {
            self.extent
        } else {
            self.values.len()
        }
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn retain(&mut self, mut keep: impl FnMut(Id, &T) -> bool) {
        let extent = self.len();
        let old_ids = self.ids.take();
        let mut ids = Vec::new();
        self.values = std::mem::take(&mut self.values)
            .into_iter()
            .enumerate()
            .filter_map(|(index, value)| {
                let id = old_ids.as_ref().map_or(index as u32, |ids| ids[index]);
                if keep(Id::from_u32(id), &value) {
                    ids.push(id);
                    Some(value)
                } else {
                    None
                }
            })
            .collect();
        self.ids = Some(ids);
        self.extent = extent;
    }
}
impl<Id: Idx, T> Index<Id> for Arena<Id, T> {
    type Output = T;
    fn index(&self, id: Id) -> &T {
        self.get(id).expect("linked HIR node")
    }
}

impl<Id: Idx, T> std::ops::IndexMut<Id> for Arena<Id, T> {
    fn index_mut(&mut self, id: Id) -> &mut T {
        self.get_mut(id).expect("linked HIR node")
    }
}
