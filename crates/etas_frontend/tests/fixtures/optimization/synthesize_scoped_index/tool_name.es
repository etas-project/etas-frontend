module app.main;

flow scenario_name() -> string {
    return "tool_name";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["open-scope", "load-symbols", "resolve", "validate", "close-scope"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow target_key(name: string) -> string {
    return name + "-target";
}

flow main() -> bool {
    let name = scenario_name();
    let target = target_key(name);
    let audit = build_audit(target);
    var found = false;
    for key in ["root", "workspace", "module", "flow", "tool_name-target", "block", "closure", "fallback"] limit Iterations(8) {
        if key == "tool_name-target" {
            found = true;
        }
    }
    if audit == "" {
        return false;
    }
    return found;
}
