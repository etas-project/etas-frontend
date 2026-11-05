module app.main;

flow qualify_projection(value: string) -> string {
    let namespace = "agent-observation/";
    return namespace + value;
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow audit_complete(log: string) -> bool {
    return log != "";
}

flow main() -> string {
    let projection = qualify_projection("trace_events");
    var audit = "";
    for stage in ["load", "decode", "validate", "project", "publish"] limit Iterations(5) {
        audit = append_audit(audit, projection + ":" + stage);
    }

    var identifiers = "";
    var metadata = "";
    for record in ["trace_events-01", "trace_events-02", "trace_events-03", "trace_events-04", "trace_events-05", "trace_events-06", "trace_events-07", "trace_events-08"] limit Iterations(8) {
        identifiers = identifiers + record;
    }
    for descriptor in ["trace_events-01", "trace_events-02", "trace_events-03", "trace_events-04", "trace_events-05", "trace_events-06", "trace_events-07", "trace_events-08"] limit Iterations(8) {
        metadata = metadata + descriptor;
    }

    if audit_complete(audit) {
        return audit + ":" + identifiers + ":" + metadata;
    }
    return "";
}
