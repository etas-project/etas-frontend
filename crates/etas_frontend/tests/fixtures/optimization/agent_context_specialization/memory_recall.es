module app.main;

type AgentObservationProjection;
type AgentToolSchemaSet;
type AgentEventState;
type DeterministicAgentDispatch;
type AgentThresholdGuard;

type Observation = { answer: string, trace: string, retries: string, metadata: string };
type ToolSchemas = { selected: string, memory: string, policy: string, publish: string };
type EventState = { current: string, stale: string, duplicate: string, debug: string };
type Dispatch = { selected: string, fallback: string, expensive: string, legacy: string };

@optimization([AgentObservationProjection])
flow project_observation(answer: string, trace: string, retries: string, metadata: string) -> Observation {
    return Observation { answer = answer, trace = trace, retries = retries, metadata = metadata };
}

@optimization([AgentToolSchemaSet])
flow select_tool_schema(selected: string, memory: string, policy: string, publish: string) -> ToolSchemas {
    return ToolSchemas { selected = selected, memory = memory, policy = policy, publish = publish };
}

@optimization([AgentEventState])
flow select_agent_event(current: string, stale: string, duplicate: string, debug: string) -> EventState {
    return EventState { current = current, stale = stale, duplicate = duplicate, debug = debug };
}

@optimization([DeterministicAgentDispatch])
flow select_dispatch(selected: string, fallback: string, expensive: string, legacy: string) -> Dispatch {
    return Dispatch { selected = selected, fallback = fallback, expensive = expensive, legacy = legacy };
}

@optimization([AgentThresholdGuard])
flow threshold_reached(value: bool) -> bool { return value; }

flow scenario_name() -> string { return "memory_recall"; }

flow append_audit(log: string, event: string) -> string { return log + "|" + event; }

flow build_audit(name: string) -> string {
    var audit = "";
    for stage in ["query", "rank", "recall", "project", "verify"] limit Iterations(5) {
        audit = append_audit(audit, name + ":" + stage);
    }
    return audit;
}

flow main() -> string {
    let name = scenario_name();
    let audit = build_audit(name);
    let answer = project_observation("checkpoint-9", "vector-search-trace", "recall-retry-history", "embedding,namespace,version,score").answer;
    let schema = select_tool_schema("memory-lookup-schema", "memory-write-schema", "memory-policy-schema", "memory-result-schema").selected;
    let event = select_agent_event("memory-v9", "memory-v4", "memory-v9-copy", "memory-debug").current;
    let route = select_dispatch("vector-route", "keyword-route", "rerank-route", "archive-route").selected;
    if threshold_reached(true) {
        return audit + ":" + answer + ":" + schema + ":" + event + ":" + route;
    } else {
        return name + ":unreachable-threshold-branch";
    }
}
