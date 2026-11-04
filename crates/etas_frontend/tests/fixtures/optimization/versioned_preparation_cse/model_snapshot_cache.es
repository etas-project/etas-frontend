flow load_model_snapshot(model: string) -> string { return "snapshot:" + model; }
flow main() -> string {
    let request = load_model_snapshot("m3.1");
    let fallback = load_model_snapshot("m3.1");
    return request + fallback;
}
