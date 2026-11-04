flow snapshot_plugin_tools(epoch: string) -> string { return "plugins:" + epoch; }
flow main() -> string {
    var turns = "";
    for _ in [1, 2, 3] limit Iterations(3) {
        let tools = snapshot_plugin_tools("generation-7");
        turns = turns + tools;
    }
    return turns;
}
