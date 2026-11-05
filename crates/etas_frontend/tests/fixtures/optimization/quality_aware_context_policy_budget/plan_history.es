module app.main;

type QualityContext;

@optimization([QualityContext])
flow quality_context(value: string) -> string {
    return value;
}

flow scenario_name() -> string {
    return "plan_history";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["collect", "score", "deduplicate", "budget", "publish"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow fallback(name: string) -> string {
    return name + " fallback";
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    let context = quality_context("obsolete greeting stale plan old retry duplicate tool output superseded policy weak memory unrelated file previous model response verified user request current repository state failing test exact compiler diagnostic preferred implementation next action");
    if audit == "" {
        return fallback(name);
    }
    return context;
}
