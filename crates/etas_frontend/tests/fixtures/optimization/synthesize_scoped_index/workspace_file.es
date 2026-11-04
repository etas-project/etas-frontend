flow main() -> bool { var found = false; for file in ["Cargo.toml", "src/lib.rs", "src/main.rs", "README.md"] limit Iterations(4) { if file == "src/lib.rs" { found = true; } } return found; }
