module app.main;

flow qualify_scope(scope: string) -> string {
    let tenant = "tenant-hk";
    let separator = "/";
    return tenant + separator + scope;
}

flow append_audit(log: string, event: string) -> string {
    let separator = "|";
    return log + separator + event;
}

flow audit_ready(log: string) -> bool {
    return log != "";
}

flow main() -> bool {
    let scope = qualify_scope("plugin_states");
    var audit = "";
    for event in ["load", "authorize", "scan", "compare", "publish"] limit Iterations(5) {
        audit = append_audit(audit, scope + ":" + event);
    }

    var first = false;
    var second = false;
    for key_a in ["plugin_states_primary", "stale", "queued", "active", "retry", "plugin_states_published", "archived", "expired"] limit Iterations(8) {
        if key_a == "plugin_states_primary" {
            first = true;
        }
    }
    for key_b in ["plugin_states_primary", "stale", "queued", "active", "retry", "plugin_states_published", "archived", "expired"] limit Iterations(8) {
        if key_b == "plugin_states_published" {
            second = true;
        }
    }

    if !audit_ready(audit) {
        return false;
    }
    if first {
        return second;
    }
    return false;
}
