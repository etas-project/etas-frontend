module app.main;

type ReusableResourceFactory;

@optimization([ReusableResourceFactory])
flow acquire_resource(name: string) -> string {
    return name + ":leased";
}

flow scenario_name() -> string {
    return "tool_host";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["configure", "authorize", "acquire", "execute", "release"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    var output = "";
    for task in ["request-01", "request-02", "request-03", "request-04", "request-05", "request-06", "request-07", "request-08"] limit Iterations(8) {
        let resource = acquire_resource("tool_host-shared");
        output = output + resource + ":" + task;
    }
    if audit == "" {
        return "";
    }
    return audit + ":" + output;
}
