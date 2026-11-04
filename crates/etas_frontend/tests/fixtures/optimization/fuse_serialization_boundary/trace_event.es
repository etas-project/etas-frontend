import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"span\":\"tool.call\",\"elapsed_ms\":17}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
