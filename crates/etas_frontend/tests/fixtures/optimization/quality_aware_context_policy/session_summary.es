module app.main;

type QualityContext;

@optimization([QualityContext])
flow build_quality_context(value: string) -> string {
    let separator = " | ";
    return value + separator + value + separator + value;
}

flow scenario_name() -> string {
    return "session_summary";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["collect", "rank", "trim", "answer", "verify"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow selected_answer(name: string) -> string {
    return name + ":current verified answer";
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    let discarded_context = build_quality_context("stale trace old retry unrelated files superseded plan low confidence retrieval");
    if audit == "" {
        return "";
    }
    return selected_answer(name);
}
