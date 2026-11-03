module app;

type Seed = { number: i32 };
type Output = { number: i32 };
type Unused = { number: i32 };
spec Label { flow label(self) -> string; }

flow main() -> string {
    return Seed { number = 3 }.advance().label();
}

impl Seed {
    public flow advance(self) -> Output { return Output { number = self.number + 1 }; }
}
impl Output ~ Label {
    flow label(self) -> string { return "selected"; }
}
impl Unused ~ Label {
    flow label(self) -> string { return 9; }
}
