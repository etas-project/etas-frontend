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
    let projection = qualify_projection("policy_clauses");
    var audit = "";
    for stage in ["load", "decode", "validate", "project", "publish"] {
        audit = append_audit(audit, projection + ":" + stage);
    }

    var identifiers = "";
    var metadata = "";
    for record in ["policy_clauses-01", "policy_clauses-02", "policy_clauses-03", "policy_clauses-04", "policy_clauses-05", "policy_clauses-06", "policy_clauses-07", "policy_clauses-08"] {
        identifiers = identifiers + record;
    }
    for descriptor in ["policy_clauses-01", "policy_clauses-02", "policy_clauses-03", "policy_clauses-04", "policy_clauses-05", "policy_clauses-06", "policy_clauses-07", "policy_clauses-08"] {
        metadata = metadata + descriptor;
    }

    if audit_complete(audit) {
        return audit + ":" + identifiers + ":" + metadata;
    }
    return "";
}
