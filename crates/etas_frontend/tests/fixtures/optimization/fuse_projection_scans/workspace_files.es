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
    let projection = qualify_projection("workspace_files");
    var audit = "";
    for stage in ["load", "decode", "validate", "project", "publish"] limit Iterations(5) {
        audit = append_audit(audit, projection + ":" + stage);
    }

    var identifiers = "";
    var metadata = "";
    for record in ["workspace_files-01", "workspace_files-02", "workspace_files-03", "workspace_files-04", "workspace_files-05", "workspace_files-06", "workspace_files-07", "workspace_files-08"] limit Iterations(8) {
        identifiers = identifiers + record;
    }
    for descriptor in ["workspace_files-01", "workspace_files-02", "workspace_files-03", "workspace_files-04", "workspace_files-05", "workspace_files-06", "workspace_files-07", "workspace_files-08"] limit Iterations(8) {
        metadata = metadata + descriptor;
    }

    if audit_complete(audit) {
        return audit + ":" + identifiers + ":" + metadata;
    }
    return "";
}
