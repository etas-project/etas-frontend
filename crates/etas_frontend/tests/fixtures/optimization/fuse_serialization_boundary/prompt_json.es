import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"role\":\"system\",\"content\":\"be concise\"}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
