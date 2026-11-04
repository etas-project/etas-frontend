flow slice_context(value: string) -> string { return value + ":recent"; } flow main() -> string { return slice_context("history"); }
