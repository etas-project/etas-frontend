flow main() -> bool { var found = false; for key in ["session", "cursor", "summary", "checkpoint"] limit Iterations(4) { if key == "summary" { found = true; } } return found; }
