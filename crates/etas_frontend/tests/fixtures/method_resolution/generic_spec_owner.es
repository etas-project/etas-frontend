module app.main;

effect Probe { action hit<T>() -> unit; }
spec Read<T> { flow read(self) -> T ![Probe.hit<T>]; }
type Container<T> = { value: T };

impl Container<T> ~ Read<T> {
    public flow read(self) -> T ![Probe.hit<T>] {
        perform Probe.hit<T>();
        return self.value;
    }
}

flow load<S ~ Read<i32>>(value: S) -> i32 {
    return value.read();
}

flow main() -> i32 {
    return load(Container<i32> { value = 7 });
}
