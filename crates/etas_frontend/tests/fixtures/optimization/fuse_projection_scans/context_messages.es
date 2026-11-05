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
    let projection = qualify_projection("context_messages");
    var audit = "";
    for stage in ["load", "decode", "validate", "project", "publish"] limit Iterations(5) {
        audit = append_audit(audit, projection + ":" + stage);
    }

    var identifiers = "";
    var metadata = "";
    for record in ["context_messages-01", "context_messages-02", "context_messages-03", "context_messages-04", "context_messages-05", "context_messages-06", "context_messages-07", "context_messages-08"] limit Iterations(8) {
        identifiers = identifiers + record;
    }
    for descriptor in ["context_messages-01", "context_messages-02", "context_messages-03", "context_messages-04", "context_messages-05", "context_messages-06", "context_messages-07", "context_messages-08"] limit Iterations(8) {
        metadata = metadata + descriptor;
    }

    if audit_complete(audit) {
        return audit + ":" + identifiers + ":" + metadata;
    }
    return "";
}
