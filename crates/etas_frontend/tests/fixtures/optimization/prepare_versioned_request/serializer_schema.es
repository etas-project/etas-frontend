flow compile_output_schema(name: string) -> string { return name + ":strict-json"; }
flow main() -> string {
    var outputs = "";
    for _ in [1, 2, 3, 4, 5] limit Iterations(5) {
        let schema = compile_output_schema("evidence-pack");
        outputs = outputs + schema;
    }
    return outputs;
}
