flow copy_workspace_view(value: string) -> string { return value; } flow main() -> string { let current = "src:tests:docs"; return copy_workspace_view(current); }
