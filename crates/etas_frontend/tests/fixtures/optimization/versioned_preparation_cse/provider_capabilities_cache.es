flow map_provider_capabilities(provider: string) -> string { return provider + ":tools-json"; }
flow main() -> string {
    let direct = map_provider_capabilities("minimax");
    let retry = map_provider_capabilities("minimax");
    return direct + retry;
}
