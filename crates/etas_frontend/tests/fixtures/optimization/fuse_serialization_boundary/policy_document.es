import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"network\":\"restricted\",\"approval\":\"on-request\"}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
