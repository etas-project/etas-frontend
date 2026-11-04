module method_resolution.generic_projection;

flow first_key(store: Store<string, string>) -> string {
    let first = store.page(None(), 50);
    let second = store.page(first.cursor, 50);
    return second.entries[0].key;
}
