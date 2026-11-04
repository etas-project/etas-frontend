flow prepare_tool_schema(name: string) -> string { return name + ":schema"; }
flow main() -> string {
    var payload = "";
    for _ in [1, 2, 3, 4] limit Iterations(4) {
        let schema = prepare_tool_schema("workspace-tools");
        payload = payload + schema;
    }
    return payload;
}
