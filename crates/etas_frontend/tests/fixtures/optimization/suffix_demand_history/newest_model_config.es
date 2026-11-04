flow main() -> string { var latest = ""; for config in ["m2", "m2.5", "m3", "m3.1-flash"] limit Iterations(4) { latest = config; } return latest; }
