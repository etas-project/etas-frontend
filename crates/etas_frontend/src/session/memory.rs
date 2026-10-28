use std::{cell::RefCell, rc::Rc};

use etas_cache::ProjectRevision;
use etas_hir::HirTreeIndex;
use etas_utils::{PassContext, PassDescriptor, PassInstrumentation, PassResult, UnitKey};

use crate::ProjectContext;

/// Requested allocator bytes, not RSS, serialized bytes, or allocator overhead.
/// Counters are monotonic except live_bytes. A host should sample its allocator
/// without allocating; concurrent work is included if the allocator is global.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocationSnapshot {
    pub live_bytes: u64,
    pub allocated_bytes: u64,
    pub allocations: u64,
    pub peak_live_bytes: u64,
}

/// Optional host instrumentation. The frontend never installs a global allocator.
#[derive(Clone, Copy, Debug)]
pub struct FrontendMemoryProbe(pub fn() -> AllocationSnapshot);

impl FrontendMemoryProbe {
    pub fn sample(self) -> AllocationSnapshot {
        (self.0)()
    }
}

#[derive(Clone, Debug)]
pub struct PassMemorySample {
    pub pass: &'static str,
    pub unit: Option<UnitKey>,
    pub before: AllocationSnapshot,
    pub after: AllocationSnapshot,
    pub hir_index_builds: u64,
}

#[derive(Clone, Debug, Default)]
pub struct CheckMemoryReport {
    pub before: Option<AllocationSnapshot>,
    pub after: Option<AllocationSnapshot>,
    pub passes: Vec<PassMemorySample>,
    pub hir_index_builds: u64,
    /// Session-owned last-good revision. Caller-held revisions are not counted.
    pub retained_revision: Option<ProjectRevision>,
    pub cache_before: Option<etas_cache::MemoryCacheUsage>,
    pub cache_after: Option<etas_cache::MemoryCacheUsage>,
    /// Graph referenced by the last-good revision; may overlap derived caches.
    pub retained_revision_estimated_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ProjectMemoryUsage {
    pub cache: Option<etas_cache::MemoryCacheUsage>,
    pub retained_revision: Option<ProjectRevision>,
    pub retained_revision_estimated_bytes: u64,
}

#[derive(Clone)]
pub(crate) struct MemoryInstrumentation {
    probe: FrontendMemoryProbe,
    state: Rc<RefCell<InstrumentationState>>,
}

#[derive(Default)]
struct InstrumentationState {
    stack: Vec<(AllocationSnapshot, u64)>,
    samples: Vec<PassMemorySample>,
}

impl MemoryInstrumentation {
    pub(crate) fn new(probe: FrontendMemoryProbe) -> Self {
        Self {
            probe,
            state: Rc::default(),
        }
    }

    pub(crate) fn take_samples(self) -> Vec<PassMemorySample> {
        std::mem::take(&mut self.state.borrow_mut().samples)
    }
}

impl PassInstrumentation<ProjectContext> for MemoryInstrumentation {
    fn before_pass(
        &mut self,
        _pass: &PassDescriptor,
        _pass_context: &PassContext<ProjectContext>,
        _context: &ProjectContext,
    ) {
        self.state
            .borrow_mut()
            .stack
            .push((self.probe.sample(), HirTreeIndex::thread_build_count()));
    }

    fn after_pass(
        &mut self,
        pass: &PassDescriptor,
        pass_context: &PassContext<ProjectContext>,
        _result: &PassResult,
        _context: &ProjectContext,
    ) {
        let after = self.probe.sample();
        let mut state = self.state.borrow_mut();
        let (before, index_builds) = state
            .stack
            .pop()
            .expect("pass instrumentation is called in before/after pairs");
        state.samples.push(PassMemorySample {
            pass: pass.name,
            unit: pass_context.current_unit,
            before,
            after,
            hir_index_builds: HirTreeIndex::thread_build_count() - index_builds,
        });
    }
}
