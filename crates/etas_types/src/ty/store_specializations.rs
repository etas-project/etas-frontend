//! Stable slots for the structural types derived while checking a library.
//! Checked declarations and their IDs remain in the frozen prefix.
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

use super::Type;
use etas_core::memory::{MemoryVisitor, RetainedSize};

#[derive(Debug, Default)]
struct Slot<T>(OnceLock<T>);

impl<T: RetainedSize> RetainedSize for Slot<T> {
    fn visit_retained<'a>(&'a self, visitor: &mut MemoryVisitor<'a>) {
        if let Some(value) = self.0.get() {
            visitor.enqueue(value);
        }
    }
}

#[derive(Debug)]
pub(super) struct Specializations {
    pages: Box<[Slot<Box<[Slot<Type>]>>]>,
    limit: usize,
    len: AtomicUsize,
    pub(super) writer: Mutex<()>,
}

impl RetainedSize for Specializations {
    fn visit_retained<'a>(&'a self, visitor: &mut MemoryVisitor<'a>) {
        visitor.enqueue(&self.pages);
    }
}

const PAGE_SIZE: usize = 64;
impl Specializations {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            pages: (0..limit.div_ceil(PAGE_SIZE))
                .map(|_| Slot(OnceLock::new()))
                .collect(),
            limit,
            len: AtomicUsize::new(0),
            writer: Mutex::new(()),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub(super) fn get(&self, index: usize) -> Option<&Type> {
        self.pages.get(index / PAGE_SIZE)?.0.get()?[index % PAGE_SIZE]
            .0
            .get()
    }

    // The lookup index holds the writer lock from its final lookup through publication.
    pub(super) fn push(&self, ty: Type) -> Option<usize> {
        let index = self.len();
        if index == self.limit {
            return None;
        }
        let page = self.pages[index / PAGE_SIZE]
            .0
            .get_or_init(|| (0..PAGE_SIZE).map(|_| Slot(OnceLock::new())).collect());
        page[index % PAGE_SIZE]
            .0
            .set(ty)
            .expect("fresh specialization slot");
        self.len.store(index + 1, Ordering::Release);
        Some(index)
    }
}
