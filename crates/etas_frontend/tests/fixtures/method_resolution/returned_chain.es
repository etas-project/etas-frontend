module method_resolution.returned_chain;

spec Number { flow number(self) -> i32 ![]; }
type Packet = { value: i32 };

impl Packet {
    public flow copy(self) -> Packet ![] { return self; }
}

impl Packet ~ Number {
    flow number(self) -> i32 ![] { return self.value; }
}

flow main() -> i32 ![] {
    return Packet { value = 42 }.copy().number();
}
