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
    let projection = qualify_projection("repo_paths");
    var audit = "";
    for stage in ["load", "decode", "validate", "project", "publish"] limit Iterations(5) {
        audit = append_audit(audit, projection + ":" + stage);
    }

    var identifiers = "";
    var metadata = "";
    for record in ["repo_paths-01", "repo_paths-02", "repo_paths-03", "repo_paths-04", "repo_paths-05", "repo_paths-06", "repo_paths-07", "repo_paths-08"] limit Iterations(8) {
        identifiers = identifiers + record;
    }
    for descriptor in ["repo_paths-01", "repo_paths-02", "repo_paths-03", "repo_paths-04", "repo_paths-05", "repo_paths-06", "repo_paths-07", "repo_paths-08"] limit Iterations(8) {
        metadata = metadata + descriptor;
    }

    if audit_complete(audit) {
        return audit + ":" + identifiers + ":" + metadata;
    }
    return "";
}
