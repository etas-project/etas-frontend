use etas_utils::JoinSemiLattice;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control<D> {
    normal: Option<D>,
    returned: Option<D>,
    broken: Option<D>,
    continued: Option<D>,
    resumed: Option<D>,
    finished: Option<D>,
    errored: Option<D>,
}

impl<D> Default for Control<D> {
    fn default() -> Self {
        Self {
            normal: None,
            returned: None,
            broken: None,
            continued: None,
            resumed: None,
            finished: None,
            errored: None,
        }
    }
}

impl<D> Control<D>
where
    D: Clone + JoinSemiLattice,
{
    pub fn bottom() -> Self {
        Self::default()
    }

    pub fn normal(state: D) -> Self {
        let mut control = Self::bottom();
        control.normal = Some(state);
        control
    }

    pub fn returning(state: D) -> Self {
        let mut control = Self::bottom();
        control.returned = Some(state);
        control
    }

    pub fn breaking(state: D) -> Self {
        let mut control = Self::bottom();
        control.broken = Some(state);
        control
    }

    pub fn continuing(state: D) -> Self {
        let mut control = Self::bottom();
        control.continued = Some(state);
        control
    }

    pub fn resuming(state: D) -> Self {
        let mut control = Self::bottom();
        control.resumed = Some(state);
        control
    }

    pub fn finishing(state: D) -> Self {
        let mut control = Self::bottom();
        control.finished = Some(state);
        control
    }

    pub fn error(state: D) -> Self {
        let mut control = Self::bottom();
        control.errored = Some(state);
        control
    }

    pub fn normal_state(&self) -> Option<&D> {
        self.normal.as_ref()
    }

    pub fn take_normal_state(&mut self) -> Option<D> {
        self.normal.take()
    }

    pub fn return_state(&self) -> Option<&D> {
        self.returned.as_ref()
    }

    pub fn break_state(&self) -> Option<&D> {
        self.broken.as_ref()
    }

    pub fn continue_state(&self) -> Option<&D> {
        self.continued.as_ref()
    }

    pub fn resume_state(&self) -> Option<&D> {
        self.resumed.as_ref()
    }

    pub fn finish_state(&self) -> Option<&D> {
        self.finished.as_ref()
    }

    pub fn error_state(&self) -> Option<&D> {
        self.errored.as_ref()
    }

    pub fn has_normal(&self) -> bool {
        self.normal.is_some()
    }

    pub fn join_assign(&mut self, other: &Self) -> bool {
        let mut changed = false;
        changed |= join_slot(&mut self.normal, &other.normal);
        changed |= join_slot(&mut self.returned, &other.returned);
        changed |= join_slot(&mut self.broken, &other.broken);
        changed |= join_slot(&mut self.continued, &other.continued);
        changed |= join_slot(&mut self.resumed, &other.resumed);
        changed |= join_slot(&mut self.finished, &other.finished);
        changed |= join_slot(&mut self.errored, &other.errored);
        changed
    }

    pub fn join_normal(&mut self, state: D) -> bool {
        join_slot(&mut self.normal, &Some(state))
    }

    pub fn join_return(&mut self, state: D) -> bool {
        join_slot(&mut self.returned, &Some(state))
    }

    pub fn join_break(&mut self, state: D) -> bool {
        join_slot(&mut self.broken, &Some(state))
    }

    pub fn join_continue(&mut self, state: D) -> bool {
        join_slot(&mut self.continued, &Some(state))
    }

    pub fn join_resume(&mut self, state: D) -> bool {
        join_slot(&mut self.resumed, &Some(state))
    }

    pub fn join_finish(&mut self, state: D) -> bool {
        join_slot(&mut self.finished, &Some(state))
    }

    pub fn join_error(&mut self, state: D) -> bool {
        join_slot(&mut self.errored, &Some(state))
    }

    pub fn with_normal_processed(mut self, f: impl FnOnce(D) -> Control<D>) -> Control<D> {
        let Some(normal) = self.normal.take() else {
            return self;
        };
        self.join_assign(&f(normal));
        self
    }

    pub fn map_states(mut self, mut f: impl FnMut(D) -> D) -> Self {
        self.normal = self.normal.map(&mut f);
        self.returned = self.returned.map(&mut f);
        self.broken = self.broken.map(&mut f);
        self.continued = self.continued.map(&mut f);
        self.resumed = self.resumed.map(&mut f);
        self.finished = self.finished.map(&mut f);
        self.errored = self.errored.map(f);
        self
    }

    pub fn into_joined_domain(self) -> D {
        let mut joined = D::bottom();
        if let Some(state) = self.normal {
            joined.join_assign(&state);
        }
        if let Some(state) = self.returned {
            joined.join_assign(&state);
        }
        if let Some(state) = self.broken {
            joined.join_assign(&state);
        }
        if let Some(state) = self.continued {
            joined.join_assign(&state);
        }
        if let Some(state) = self.resumed {
            joined.join_assign(&state);
        }
        if let Some(state) = self.finished {
            joined.join_assign(&state);
        }
        if let Some(state) = self.errored {
            joined.join_assign(&state);
        }
        joined
    }
}

fn join_slot<D>(slot: &mut Option<D>, other: &Option<D>) -> bool
where
    D: Clone + JoinSemiLattice,
{
    let Some(other) = other else {
        return false;
    };
    match slot {
        Some(current) => current.join_assign(other),
        None => {
            *slot = Some(other.clone());
            true
        }
    }
}
