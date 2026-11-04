flow main() -> bool { var found = false; for capability in ["text", "tools", "json", "vision"] limit Iterations(4) { if capability == "json" { found = true; } } return found; }
