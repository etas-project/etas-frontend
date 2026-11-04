flow main() -> bool { var found = false; for id in ["fs", "git", "browser", "database"] limit Iterations(4) { if id == "browser" { found = true; } } return found; }
