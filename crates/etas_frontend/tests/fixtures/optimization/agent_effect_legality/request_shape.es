flow shape_request(model: string) -> string { return model + ":json"; } flow main() -> string { return shape_request("m3"); }
