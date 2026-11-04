flow compile_tool_schema(catalog: string) -> string { return "schema:" + catalog; }
flow main() -> string {
    let prompt = compile_tool_schema("workspace");
    let request = compile_tool_schema("workspace");
    return prompt + request;
}
