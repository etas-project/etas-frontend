module app;

type Container<T> = { value: T };

impl Container<T> {
    public flow get(self) -> T { return self.value; }
    public flow replace<T>(self, value: T) -> Container<T> {
        return Container<T> { value = value };
    }
}

flow main() -> string {
    return Container<i32> { value = 7 }.replace("changed").get();
}
