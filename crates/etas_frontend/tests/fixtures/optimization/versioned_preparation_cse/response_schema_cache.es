flow normalize_response_schema(name: string) -> string { return name + ":strict"; }
flow main() -> string {
    let validation = normalize_response_schema("evidence");
    let decoding = normalize_response_schema("evidence");
    return validation + decoding;
}
