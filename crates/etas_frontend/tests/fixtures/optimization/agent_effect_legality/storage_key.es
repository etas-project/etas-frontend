flow normalize_key(value: string) -> string { return value + ":v1"; } flow main() -> string { return normalize_key("session"); }
