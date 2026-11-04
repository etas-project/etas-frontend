flow project_policy(scope: string) -> string { return "policy:" + scope; }
flow main() -> string {
    let dispatch = project_policy("sandbox");
    let audit = project_policy("sandbox");
    return dispatch + audit;
}
