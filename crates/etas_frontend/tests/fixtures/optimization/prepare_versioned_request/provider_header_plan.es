module app.main;

flow prepare_provider_header_plan(profile: string) -> string {
    let version = ":revision-7";
    let encoding = ":json";
    return profile + version + encoding;
}

flow scenario_name() -> string {
    return "provider_header_plan";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["resolve", "validate", "prepare", "dispatch", "observe"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    var payload = "";
    for revision in ["r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8"] limit Iterations(8) {
        let prepared = prepare_provider_header_plan("provider_header_plan-stable-profile");
        payload = payload + prepared + ":" + revision;
    }
    if audit == "" {
        return "";
    }
    return audit + ":" + payload;
}
