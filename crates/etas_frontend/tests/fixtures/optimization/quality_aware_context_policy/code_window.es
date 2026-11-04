flow build_code_window(value: string) -> string { return value + value + value; } flow main() -> string { let context_wide_window = build_code_window("l1:l2:l3:l4"); return "l3"; }
