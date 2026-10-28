use etas_utils::JoinSemiLattice;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control<D> {
    normal: Option<Box<D>>,
    returned: Option<Box<D>>,
    broken: Option<Box<D>>,
    continued: Option<Box<D>>,
    resumed: Option<Box<D>>,
    finished: Option<Box<D>>,
    errored: Option<Box<D>>,
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
        control.normal = Some(Box::new(state));
        control
    }

    pub fn returning(state: D) -> Self {
        let mut control = Self::bottom();
        control.returned = Some(Box::new(state));
        control
    }

    pub fn breaking(state: D) -> Self {
        let mut control = Self::bottom();
        control.broken = Some(Box::new(state));
        control
    }

    pub fn continuing(state: D) -> Self {
        let mut control = Self::bottom();
        control.continued = Some(Box::new(state));
        control
    }

    pub fn resuming(state: D) -> Self {
        let mut control = Self::bottom();
        control.resumed = Some(Box::new(state));
        control
    }

    pub fn finishing(state: D) -> Self {
        let mut control = Self::bottom();
        control.finished = Some(Box::new(state));
        control
    }

    pub fn error(state: D) -> Self {
        let mut control = Self::bottom();
        control.errored = Some(Box::new(state));
        control
    }

    pub fn normal_state(&self) -> Option<&D> {
        self.normal.as_deref()
    }

    pub fn take_normal_state(&mut self) -> Option<D> {
        self.normal.take().map(|state| *state)
    }

    pub fn return_state(&self) -> Option<&D> {
        self.returned.as_deref()
    }

    pub fn break_state(&self) -> Option<&D> {
        self.broken.as_deref()
    }

    pub fn continue_state(&self) -> Option<&D> {
        self.continued.as_deref()
    }

    pub fn resume_state(&self) -> Option<&D> {
        self.resumed.as_deref()
    }

    pub fn finish_state(&self) -> Option<&D> {
        self.finished.as_deref()
    }

    pub fn error_state(&self) -> Option<&D> {
        self.errored.as_deref()
    }

    pub fn take_error_state(&mut self) -> Option<D> {
        self.errored.take().map(|state| *state)
    }

    pub fn has_normal(&self) -> bool {
        self.normal.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.normal.is_none()
            && self.returned.is_none()
            && self.broken.is_none()
            && self.continued.is_none()
            && self.resumed.is_none()
            && self.finished.is_none()
            && self.errored.is_none()
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
        join_state(&mut self.normal, state)
    }

    pub fn join_return(&mut self, state: D) -> bool {
        join_state(&mut self.returned, state)
    }

    pub fn join_break(&mut self, state: D) -> bool {
        join_state(&mut self.broken, state)
    }

    pub fn join_continue(&mut self, state: D) -> bool {
        join_state(&mut self.continued, state)
    }

    pub fn join_resume(&mut self, state: D) -> bool {
        join_state(&mut self.resumed, state)
    }

    pub fn join_finish(&mut self, state: D) -> bool {
        join_state(&mut self.finished, state)
    }

    pub fn join_error(&mut self, state: D) -> bool {
        join_state(&mut self.errored, state)
    }

    pub fn with_normal_processed(mut self, f: impl FnOnce(D) -> Control<D>) -> Control<D> {
        let Some(normal) = self.normal.take() else {
            return self;
        };
        self.join_assign(&f(*normal));
        self
    }

    pub fn try_with_normal_processed<E>(
        mut self,
        f: impl FnOnce(D) -> Result<Control<D>, E>,
    ) -> Result<Control<D>, E> {
        let Some(normal) = self.normal.take() else {
            return Ok(self);
        };
        self.join_assign(&f(*normal)?);
        Ok(self)
    }

    pub fn map_states(mut self, mut f: impl FnMut(D) -> D) -> Self {
        self.normal = map_slot(self.normal, &mut f);
        self.returned = map_slot(self.returned, &mut f);
        self.broken = map_slot(self.broken, &mut f);
        self.continued = map_slot(self.continued, &mut f);
        self.resumed = map_slot(self.resumed, &mut f);
        self.finished = map_slot(self.finished, &mut f);
        self.errored = map_slot(self.errored, &mut f);
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

fn join_slot<D>(slot: &mut Option<Box<D>>, other: &Option<Box<D>>) -> bool
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

fn join_state<D: Clone + JoinSemiLattice>(slot: &mut Option<Box<D>>, other: D) -> bool {
    match slot {
        Some(current) => current.join_assign(&other),
        None => {
            *slot = Some(Box::new(other));
            true
        }
    }
}

fn map_slot<D>(slot: Option<Box<D>>, f: &mut impl FnMut(D) -> D) -> Option<Box<D>> {
    slot.map(|mut state| {
        *state = f(*state);
        state
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_stack_size_is_independent_of_domain_size() {
        assert_eq!(
            std::mem::size_of::<Control<[u8; 32768]>>(),
            7 * std::mem::size_of::<usize>()
        );
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Count(u32);
    impl etas_utils::PartialOrder for Count {
        fn less_equal(&self, other: &Self) -> bool {
            self.0 <= other.0
        }
    }
    impl JoinSemiLattice for Count {
        fn bottom() -> Self {
            Self(0)
        }
        fn join_assign(&mut self, other: &Self) -> bool {
            let changed = other.0 > self.0;
            self.0 = self.0.max(other.0);
            changed
        }
    }
    #[test]
    fn mapping_and_normal_processing_preserve_all_non_normal_exits() {
        let mut control = Control::normal(Count(2));
        control.join_return(Count(3));
        control.join_break(Count(4));
        control.join_continue(Count(5));
        control.join_resume(Count(6));
        control.join_finish(Count(7));
        control.join_error(Count(8));
        let mut mapped = control.map_states(|value| Count(value.0 + 1));
        assert_eq!(mapped.take_normal_state(), Some(Count(3)));
        let mapped = mapped.with_normal_processed(|_| panic!("no normal state remains"));
        assert_eq!(mapped.return_state(), Some(&Count(4)));
        assert_eq!(mapped.break_state(), Some(&Count(5)));
        assert_eq!(mapped.continue_state(), Some(&Count(6)));
        assert_eq!(mapped.resume_state(), Some(&Count(7)));
        assert_eq!(mapped.finish_state(), Some(&Count(8)));
        assert_eq!(mapped.error_state(), Some(&Count(9)));
        assert_eq!(mapped.into_joined_domain(), Count(9));
    }
}
