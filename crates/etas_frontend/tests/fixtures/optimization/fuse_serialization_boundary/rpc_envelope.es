import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/list\"}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
