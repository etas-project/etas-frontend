flow main() -> bool { var found = false; for name in ["read", "write", "search", "patch"] limit Iterations(4) { if name == "patch" { found = true; } } return found; }
