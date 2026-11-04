import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"thread_id\":\"t-42\",\"version\":9,\"deleted\":false}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
