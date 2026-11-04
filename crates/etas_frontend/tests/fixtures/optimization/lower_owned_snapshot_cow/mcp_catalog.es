flow clone_mcp_catalog(value: string) -> string { return value; } flow main() -> string { let current = "fs:git:browser"; return clone_mcp_catalog(current); }
