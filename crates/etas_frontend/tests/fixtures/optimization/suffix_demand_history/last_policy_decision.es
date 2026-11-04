flow main() -> string { var latest = ""; for decision in ["review", "allow-read", "allow-test", "allow-merge"] limit Iterations(4) { latest = decision; } return latest; }
