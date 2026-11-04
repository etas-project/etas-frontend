import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"plugin\":\"github\",\"permissions\":[\"contents:read\"]}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
