flow build_repo_map(revision: string) -> string { return "map:" + revision; }
flow main() -> string {
    let primary = build_repo_map("git-a19");
    let retry = build_repo_map("git-a19");
    return primary + retry;
}
