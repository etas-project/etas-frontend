flow build_retrieval_pack(value: string) -> string { return value + value + value; } flow main() -> string { let retrieval_broad = build_retrieval_pack("d1:d2:d3:d4"); return "d1"; }
