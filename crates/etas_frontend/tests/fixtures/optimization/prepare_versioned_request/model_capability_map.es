flow map_capabilities(model: string) -> string { return model + ":json-tools"; }
flow main() -> string {
    var turns = "";
    for _ in [1, 2, 3, 4] limit Iterations(4) {
        let capabilities = map_capabilities("reasoning-model");
        turns = turns + capabilities;
    }
    return turns;
}
