flow compute_route_plan(profile: string) -> string { return "route:" + profile; }
flow main() -> string {
    let request = compute_route_plan("low-latency");
    let recovery = compute_route_plan("low-latency");
    return request + recovery;
}
