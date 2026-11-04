flow checkpoint_name(value: string) -> string { return value + ":checkpoint"; } flow main() -> string { return checkpoint_name("run"); }
