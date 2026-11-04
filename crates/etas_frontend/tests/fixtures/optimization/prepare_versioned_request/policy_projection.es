flow project_environment_policy(profile: string) -> string { return "policy:" + profile; }
flow main() -> string {
    var requests = "";
    for _ in [1, 2, 3] limit Iterations(3) {
        let policy = project_environment_policy("sandboxed");
        requests = requests + policy;
    }
    return requests;
}
