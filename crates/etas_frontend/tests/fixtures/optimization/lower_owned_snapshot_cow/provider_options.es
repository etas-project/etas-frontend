flow snapshot_provider_options(value: string) -> string { return value; } flow main() -> string { let current = "retry:3"; return snapshot_provider_options(current); }
