flow build_trace_label(value: string) -> string { return value + ":span"; } flow main() -> string { return build_trace_label("agent"); }
