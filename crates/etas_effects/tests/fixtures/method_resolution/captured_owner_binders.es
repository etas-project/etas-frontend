module main;

effect Probe { action hit<T>() -> unit; }

flow emit<T>(value: T) -> unit ![Probe.hit<T>] {
    perform Probe.hit<T>();
    return;
}

type Container<T> = { value: T };

impl Container<T> {
    public flow capture<T>(self, other: T) -> (() -> unit) {
        return () => {
            emit(self.value);
            emit(other);
            return;
        };
    }

    public flow wrap(self) -> (() -> unit) {
        return self.capture("other");
    }
}

flow main() -> unit {
    let callback = Container<i32> { value = 7 }.capture("other");
    callback();
    return;
}

flow wrapped() -> unit {
    let callback = Container<i32> { value = 7 }.wrap();
    callback();
    return;
}
