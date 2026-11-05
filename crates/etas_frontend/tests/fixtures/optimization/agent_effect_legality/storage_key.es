module app.main;

flow scenario_name() -> string {
    return "storage_key";
}

flow shape_request(value: string) -> string {
    return value + ":request-v7";
}

flow build_tool_schema(value: string) -> string {
    return value + ":strict-schema";
}

flow project_policy(value: string) -> string {
    return value + ":allow-workspace";
}

flow map_capability(value: string) -> string {
    return value + ":tools-json";
}

flow normalize_storage_key(value: string) -> string {
    return "tenant-hk/" + value;
}

flow trace_label(value: string) -> string {
    return "agent/" + value + "/publish";
}

flow main() -> string {
    let name = scenario_name();
    let request = shape_request(name);
    let schema = build_tool_schema(name);
    let policy = project_policy(name);
    let capability = map_capability(name);
    let storage = normalize_storage_key(name);
    let trace = trace_label(name);
    return request + "|" + schema + "|" + policy + "|" + capability + "|" + storage + "|" + trace;
}
