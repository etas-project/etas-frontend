flow snapshot_plugin_manifest(value: string) -> string { return value; } flow main() -> string { let current = "search:enabled"; return snapshot_plugin_manifest(current); }
