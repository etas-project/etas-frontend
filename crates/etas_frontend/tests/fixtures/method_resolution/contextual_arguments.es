module main;
type Item = { value: i32 };
type Engine = { id: i32 };
type Other = { id: i32 };
impl Item { flow read_value(self) -> i32 { return self.value; } }
spec Apply {
    flow apply(self, label: string, values: Array<i32>, options: Map<string, i32>, next: (Item) -> i32 ![]) -> i32 ![];
}
impl Engine { flow copy(self) -> Engine { return self; } }
impl Engine ~ Apply {
    flow apply(self, label: string, values: Array<i32>, options: Map<string, i32>, next: (Item) -> i32 ![]) -> i32 ![] {
        return next(Item { value = 8 });
    }
}
impl Other {
    flow apply(self, label: i32, values: Array<string>, options: Map<string, string>, next: (string) -> string ![]) -> string ![] {
        return next("other");
    }
}
flow main() -> i32 ![] {
    return Engine { id = 1 }.copy().apply(
        values = [],
        next = item => { return item.read_value(); },
        options = {},
        label = "engine",
    );
}
