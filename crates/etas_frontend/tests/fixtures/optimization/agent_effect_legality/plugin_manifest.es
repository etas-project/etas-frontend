flow map_plugin(value: string) -> string { return value + ":enabled"; } flow main() -> string { return map_plugin("search"); }
