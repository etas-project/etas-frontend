module app.main;

flow scenario_name() -> string {
    return "recent_checkpoint";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["load", "order", "validate", "select", "publish"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow fallback(name: string) -> string {
    let status = ":missing";
    return name + status;
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    var latest = "";
    for revision in ["revision-01", "revision-02", "revision-03", "revision-04", "revision-05", "revision-06", "revision-07", "recent_checkpoint-current"] limit Iterations(8) {
        latest = revision;
    }
    if audit == "" {
        return fallback(name);
    }
    return latest;
}
