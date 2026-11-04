flow main() -> string { var latest = ""; for result in ["queued", "running", "parsed", "accepted", "published"] limit Iterations(5) { latest = result; } return latest; }
