module main;

type Seed = { value: i32 };
type Output = { value: i32 };

impl Seed {
    public flow bump(self) -> Seed {
        return Seed { value = self.value + 1 };
    }
    public flow to_output(self) -> Output {
        return Output { value = self.value };
    }
}

spec Label {
    flow label(self) -> string;
}

impl Output ~ Label {
    flow label(self) -> string { return "ready"; }
}

flow ordinary() -> string {
    return Seed { value = 7 }.bump().bump().to_output().label();
}

flow selected() -> string {
    return Seed { value = 7 }.to_output()::Label.label();
}

flow generic<T ~ Label>(value: T) -> string {
    return value.label();
}
