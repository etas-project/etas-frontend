import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"name\":\"read\",\"required\":[\"path\"]}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
