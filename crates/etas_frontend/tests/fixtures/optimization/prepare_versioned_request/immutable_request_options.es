flow merge_static_options(mode: string) -> string { return mode + ":temperature-0"; }
flow main() -> string {
    var calls = "";
    for _ in [1, 2, 3, 4] limit Iterations(4) {
        let options = merge_static_options("deterministic");
        calls = calls + options;
    }
    return calls;
}
