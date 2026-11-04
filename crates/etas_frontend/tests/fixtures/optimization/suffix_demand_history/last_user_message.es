flow main() -> string { var latest = ""; for message in ["plan", "inspect", "implement", "verify"] limit Iterations(4) { latest = message; } return latest; }
