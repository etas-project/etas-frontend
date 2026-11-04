flow main() -> string { var latest = ""; for snapshot in ["catalog-1", "catalog-2", "catalog-3"] limit Iterations(3) { latest = snapshot; } return latest; }
