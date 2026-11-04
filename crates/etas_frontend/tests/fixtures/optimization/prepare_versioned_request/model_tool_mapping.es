flow map_model_tools(provider: string) -> string { return provider + ":mapped-tools"; }
flow main() -> string {
    var request = "";
    for _ in [1, 2, 3, 4, 5] limit Iterations(5) {
        let tools = map_model_tools("responses-api");
        request = request + tools;
    }
    return request;
}
