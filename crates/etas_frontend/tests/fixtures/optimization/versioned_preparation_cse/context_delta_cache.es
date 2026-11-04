flow encode_context_delta(version: string) -> string { return "delta:" + version; }
flow main() -> string {
    let first = encode_context_delta("session-42");
    let replay = encode_context_delta("session-42");
    return first + replay;
}
