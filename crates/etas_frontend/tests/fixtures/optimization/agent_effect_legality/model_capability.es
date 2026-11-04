flow map_capability(value: string) -> string { return value + ":tools"; } flow main() -> string { return map_capability("m3"); }
