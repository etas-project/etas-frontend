flow build_plan_history(value: string) -> string { return value + value + value; } flow main() -> string { let history_plans = build_plan_history("p1:p2:p3"); return "p3"; }
