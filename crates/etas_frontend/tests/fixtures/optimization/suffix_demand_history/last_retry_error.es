flow main() -> string { var latest = ""; for failure in ["timeout", "rate-limit", "overload", "resolved"] limit Iterations(4) { latest = failure; } return latest; }
