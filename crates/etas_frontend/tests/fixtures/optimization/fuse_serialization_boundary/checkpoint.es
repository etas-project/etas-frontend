import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"checkpoint\":12,\"pending\":[\"search\",\"write\"]}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
