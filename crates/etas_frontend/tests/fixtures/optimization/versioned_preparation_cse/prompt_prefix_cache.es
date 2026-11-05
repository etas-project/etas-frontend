module app.main;

flow compile_prompt_prefix_cache(version: string) -> string {
    let prefix = "prompt_prefix_cache:";
    let encoding = ":json";
    let contract = ":stable";
    return prefix + version + encoding + contract;
}

flow scenario_name() -> string {
    return "prompt_prefix_cache";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["load", "fingerprint", "prepare", "reuse", "publish"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    let first = compile_prompt_prefix_cache("revision-7");
    let replay = compile_prompt_prefix_cache("revision-7");
    if audit == "" {
        return "";
    }
    return audit + ":" + first + ":" + replay;
}
