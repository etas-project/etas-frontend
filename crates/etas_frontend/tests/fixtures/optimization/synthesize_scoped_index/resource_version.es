flow main() -> bool { var found = false; for version in ["v31", "v32", "v33", "v34", "v35"] limit Iterations(5) { if version == "v34" { found = true; } } return found; }
