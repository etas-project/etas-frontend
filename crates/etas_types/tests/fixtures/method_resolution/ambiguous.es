module main;
type Value = i32;
spec First { flow label(self) -> string; }
spec Second { flow label(self) -> string; }
impl Value ~ First { flow label(self) -> string { return "first"; } }
impl Value ~ Second { flow label(self) -> string { return "second"; } }
flow ambiguous(value: Value) -> string { return value.label(); }
flow selected(value: Value) -> string { return value::Second.label(); }
