import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"model\":\"MiniMax-M3.1-Flash\",\"stream\":false}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
