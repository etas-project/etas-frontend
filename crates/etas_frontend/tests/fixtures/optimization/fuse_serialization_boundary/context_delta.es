import std.json.JsonValue;
import std.json.parse;
import std.json.stringify;

flow main() -> JsonValue {
    let value = parse("{\"added\":[\"a.rs\",\"b.rs\"],\"removed\":[]}").unwrap();
    return parse(stringify(value).unwrap()).unwrap();
}
