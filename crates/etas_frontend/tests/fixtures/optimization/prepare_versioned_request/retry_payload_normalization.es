flow normalize_messages(messages: string) -> string { return "normalized:" + messages; }
flow main() -> string {
    var attempts = "";
    for _ in [1, 2, 3] limit Iterations(3) {
        let payload = normalize_messages("immutable-history");
        attempts = attempts + payload;
    }
    return attempts;
}
