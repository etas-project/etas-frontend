flow main() -> bool { var found = false; for id in ["req-17", "req-18", "req-19", "req-20", "req-21"] limit Iterations(5) { if id == "req-20" { found = true; } } return found; }
