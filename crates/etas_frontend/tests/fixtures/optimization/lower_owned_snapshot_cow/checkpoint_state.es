flow copy_checkpoint_state(value: string) -> string { return value; } flow main() -> string { let current = "step:9"; return copy_checkpoint_state(current); }
