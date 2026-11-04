module app.main;

spec Read { flow read(self) -> i32 ![]; }
type Selected = { value: i32 };
type Unused = { value: i32 };
impl Selected ~ Read {
    public flow read(self) -> i32 ![] { return self.value; }
}
impl Unused ~ Read {
    public flow read(self) -> i32 ![] { return "unreachable"; }
}

flow load<S ~ Read>(value: S) -> i32 ![] { return value.read(); }
type Container<T ~ Read> = { value: T };
impl Container<T> {
    public flow unpack(self) -> i32 ![] { return load(self.value); }
}
spec Named { flow name(self) -> string ![]; }
type Token = string;
type Label<T> = { value: T };
impl Label<T> ~ Named {
    public flow name(self) -> string ![] {
        let values = [self.value];
        if values.len() == 1 { return "label"; }
        return "wrong";
    }
}
flow main() -> i32 ![] {
    let named: ? ~ Named = Label<Token> { value = Token("value") };
    if named.name() != "label" { return 0; }
    return Container<Selected> { value = Selected { value = 7 } }.unpack();
}
