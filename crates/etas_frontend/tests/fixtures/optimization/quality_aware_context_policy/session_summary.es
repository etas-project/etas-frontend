flow build_session_summary(value: string) -> string { return value + value + value; } flow main() -> string { let summary_verbose = build_session_summary("turn1:turn2:turn3"); return "turn3"; }
