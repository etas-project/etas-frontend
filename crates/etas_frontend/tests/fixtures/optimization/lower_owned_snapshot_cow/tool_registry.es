flow clone_tool_registry(value: string) -> string { return value; } flow main() -> string { let current = "read:write:shell"; return clone_tool_registry(current); }
