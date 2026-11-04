flow main() -> string { var latest = ""; for root in ["/workspace", "/workspace/project", "/workspace/project/crates"] limit Iterations(3) { latest = root; } return latest; }
