flow build_tool_catalog(value: string) -> string { return value + value + value; } flow main() -> string { let context_all_tools = build_tool_catalog("read:write:shell:web"); return "read"; }
