flow compile_serializer_config(format: string) -> string { return format + ":compact"; }
flow main() -> string {
    let body = compile_serializer_config("json");
    let trace = compile_serializer_config("json");
    return body + trace;
}
