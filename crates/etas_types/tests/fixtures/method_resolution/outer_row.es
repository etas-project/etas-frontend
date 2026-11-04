module method_resolution.outer_row;

agent Worker() -> i32 ![] { return 1; }

impl agent Worker {
    flow factory<effect E>() -> (() -> unit ![E]) ![] {
        return () => { return; };
    }

    flow echo<effect E>(callback: () -> unit ![E]) -> (() -> unit ![E]) ![] {
        return callback;
    }
}

flow outer<effect E>(worker: Worker, callback: () -> unit ![Console.stdout_write, E])
    -> (() -> unit ![Console.stdout_write, E]) ![] {
    worker.factory<![Console.stdout_write, E]>();
    return worker.echo<![Console.stdout_write, E]>(callback);
}
