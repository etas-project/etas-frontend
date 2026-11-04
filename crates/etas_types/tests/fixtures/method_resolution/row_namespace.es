module method_resolution.row_namespace;

agent Worker() -> i32 ![] { return 1; }
impl agent Worker {
    flow choose<effect E, effect F>(first: () -> unit ![E], second: () -> unit ![F])
        -> (() -> unit ![E]) ![] {
        return first;
    }
}

flow outer<effect F>(worker: Worker, callback: () -> unit ![F], console: () -> unit ![Console.stdout_write])
    -> (() -> unit ![F]) ![] {
    return worker.choose<![F], ![Console.stdout_write]>(callback, console);
}

flow inline_outer<effect F>(worker: Worker, callback: () -> unit ![F], console: () -> unit ![Console.stdout_write])
    -> (() -> unit ![F]) ![] {
    return worker.choose<![F], ![Console.stdout_write]>(
        () => { callback(); return; },
        () => { console(); return; },
    );
}
