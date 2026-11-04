flow prepare_headers(provider: string) -> string { return provider + ":content-type"; }
flow main() -> string {
    var sent = "";
    for _ in [1, 2, 3, 4, 5, 6] limit Iterations(6) {
        let headers = prepare_headers("openai");
        sent = sent + headers;
    }
    return sent;
}
