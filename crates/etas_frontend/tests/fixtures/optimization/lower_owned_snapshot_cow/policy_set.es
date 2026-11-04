flow snapshot_policy_set(value: string) -> string { return value; } flow main() -> string { let current = "allow:ask"; return snapshot_policy_set(current); }
