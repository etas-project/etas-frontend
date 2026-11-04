flow main() -> string { var latest = ""; for checkpoint in ["c10", "c11", "c12", "c13", "c14", "c15"] limit Iterations(6) { latest = checkpoint; } return latest; }
