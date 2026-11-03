module method_resolution.named_bound;

type Counter = { value: i32 };

spec Add {
    flow add(self, amount: i32) -> i32;
}

impl Counter ~ Add {
    flow add(self, input: i32) -> i32 { return self.value + input; }
}

flow bounded<T ~ Add>(value: T) -> i32 {
    return value.add(amount = 2);
}

flow direct() -> i32 {
    return Counter { value = 4 }.add(amount = 3);
}

flow selected<T ~ Add>(value: T) -> i32 {
    return value::Add.add(amount = 5);
}
