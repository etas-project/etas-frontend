flow provider_header(value: string) -> string { return value + ":content-type"; } flow main() -> string { return provider_header("minimax"); }
