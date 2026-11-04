flow build_schema(tool: string) -> string { return tool + ":schema"; } flow main() -> string { return build_schema("read"); }
