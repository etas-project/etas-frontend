module app.main;
import std.io.println;

spec Named { flow name(self) -> string ![]; }
type Label = string;
type Number = i32;
impl Label ~ Named {
    flow name(self) -> string ![] {
        println("first")?;
        return "first";
    }
}
impl Number ~ Named {
    flow name(self) -> string ![] {
        println("second")?;
        return "second";
    }
}
flow main() -> usize ![] {
    let values: Array<? ~ Named> = [Label("example"), Number(7)];
    let total: usize = 0;
    for value in values {
        total = total + value.name().len();
    }
    return total;
}
