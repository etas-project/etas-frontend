module main;

effect Probe { action hit<T>() -> unit; }

flow emit<T>(value: T) -> unit ![Probe.hit<T>] {
    perform Probe.hit<T>();
    return;
}

type Container<T> = { value: T };

impl Container<T> {
    public flow both<T>(self, other: T) -> unit {
        emit(self.value);
        emit(other);
        return;
    }
}

flow main() -> unit {
    Container<i32> { value = 7 }.both("other");
    return;
}
