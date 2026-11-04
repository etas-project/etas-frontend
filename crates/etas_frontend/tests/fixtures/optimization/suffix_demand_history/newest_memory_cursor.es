flow main() -> string { var latest = ""; for cursor in ["v17", "v18", "v19", "v20", "v21"] limit Iterations(5) { latest = cursor; } return latest; }
