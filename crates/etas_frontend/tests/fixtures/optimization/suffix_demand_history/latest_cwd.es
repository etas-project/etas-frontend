flow main() -> string { var latest = ""; for cwd in ["/repo", "/repo/src", "/repo/src/app", "/repo/src/app/core"] limit Iterations(4) { latest = cwd; } return latest; }
