module main;

effect Audit { action inherent() -> unit; action selected() -> unit; }
type Start = { value: i32 };
type End = { value: i32 };

impl Start {
    public flow advance(self) -> End ![Audit.inherent] {
        perform Audit.inherent();
        return End { value = self.value + 1 };
    }
}

spec Finish {
    flow finish(self) -> i32 ![Audit.selected];
}

impl End ~ Finish {
    flow finish(self) -> i32 ![Audit.selected] {
        perform Audit.selected();
        return self.value;
    }
}

flow main() -> i32 ![Audit.inherent, Audit.selected] {
    return Start { value = 4 }.advance().finish();
}

flow explicit() -> i32 ![Audit.inherent, Audit.selected] {
    return Start { value = 4 }.advance()::Finish.finish();
}

flow invoke<T ~ Finish>(value: T) -> i32 ![Audit.selected] {
    return value.finish();
}

flow generic() -> i32 ![Audit.inherent, Audit.selected] {
    return invoke(Start { value = 4 }.advance());
}
