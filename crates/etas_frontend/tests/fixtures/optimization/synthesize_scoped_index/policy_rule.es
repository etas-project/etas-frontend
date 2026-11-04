flow main() -> bool { var found = false; for rule in ["read", "write", "network", "merge"] limit Iterations(4) { if rule == "network" { found = true; } } return found; }
