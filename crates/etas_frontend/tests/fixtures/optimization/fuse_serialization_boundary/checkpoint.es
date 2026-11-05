module app.main;

import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow scenario_name() -> string {
    return "checkpoint";
}

flow payload_text() -> string {
    return "{\"scenario\":\"checkpoint\",\"revision\":7,\"items\":[\"queued\",\"checked\",\"published\"]}";
}

flow audit_label(name: string) -> string {
    return "serialization/" + name;
}

flow decode_payload(text: string) -> JsonValue {
    return parse(text).unwrap();
}

flow cross_boundary(value: JsonValue) -> JsonValue {
    return parse(stringify(value).unwrap()).unwrap();
}

flow main() -> JsonValue {
    let name = scenario_name();
    let label = audit_label(name);
    let wire = payload_text();
    let decoded = decode_payload(wire);
    let published = cross_boundary(decoded);
    if label == "" {
        return decode_payload(wire);
    }
    return published;
}
