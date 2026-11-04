flow copy_session_state(value: string) -> string { return value; } flow main() -> string { let current = "turn:42"; return copy_session_state(current); }
