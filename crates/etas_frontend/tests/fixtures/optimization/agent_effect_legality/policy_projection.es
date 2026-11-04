flow project_policy(value: string) -> string { return value + ":allow"; } flow main() -> string { return project_policy("read"); }
