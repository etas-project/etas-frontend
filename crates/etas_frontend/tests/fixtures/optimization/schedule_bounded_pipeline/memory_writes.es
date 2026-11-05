module app.main;

type BoundedPipelineStage;

@optimization([BoundedPipelineStage])
flow stage_prepare(acc: string, item: string) -> string {
    return acc + "|prepare:" + item;
}

@optimization([BoundedPipelineStage])
flow stage_publish(acc: string, item: string) -> string {
    return acc + "|publish:" + item;
}

flow scenario_name() -> string {
    return "memory_writes";
}

flow append_audit(log: string, event: string) -> string {
    return log + "|" + event;
}

flow build_audit(name: string) -> string {
    var audit = "";
    for event in ["admit", "queue", "schedule", "drain", "complete"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + event);
    }
    return audit;
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    var prepared = "";
    var published = "";
    for task_a in ["job-01", "job-02", "job-03", "job-04", "job-05", "job-06", "job-07", "job-08"] limit Iterations(8) {
        prepared = stage_prepare(prepared, task_a);
    }
    for task_b in ["job-01", "job-02", "job-03", "job-04", "job-05", "job-06", "job-07", "job-08"] limit Iterations(8) {
        published = stage_publish(published, task_b);
    }
    return audit + ":" + prepared + ":" + published;
}
