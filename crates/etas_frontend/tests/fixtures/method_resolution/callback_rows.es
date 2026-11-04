module main;

spec Runner { flow run<effect E>(self, f: () -> unit ![E]) -> unit ![E]; }
type Worker = { id: i32 };
impl Worker ~ Runner {
    flow run<effect F>(self, callback: () -> unit ![F]) -> unit ![F] {
        callback();
        callback();
    }
}
flow main() -> unit ![Console.stdout_write] {
    let worker = Worker { id = 1 };
    let f = () => { perform Console.stdout_write("callback"); return; };
    worker.run<![Console.stdout_write]>(f);
}
