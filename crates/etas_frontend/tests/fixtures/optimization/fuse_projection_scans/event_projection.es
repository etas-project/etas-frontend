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
    let projection = qualify_projection("event_projection");
    var audit = "";
    for stage in ["load", "decode", "validate", "project", "publish"] limit Iterations(5) {
        audit = append_audit(audit, projection + ":" + stage);
    }

    var identifiers = "";
    var metadata = "";
    for record in ["event_projection-01", "event_projection-02", "event_projection-03", "event_projection-04", "event_projection-05", "event_projection-06", "event_projection-07", "event_projection-08"] limit Iterations(8) {
        identifiers = identifiers + record;
    }
    for descriptor in ["event_projection-01", "event_projection-02", "event_projection-03", "event_projection-04", "event_projection-05", "event_projection-06", "event_projection-07", "event_projection-08"] limit Iterations(8) {
        metadata = metadata + descriptor;
    }

    if audit_complete(audit) {
        return audit + ":" + identifiers + ":" + metadata;
    }
    return "";
}
