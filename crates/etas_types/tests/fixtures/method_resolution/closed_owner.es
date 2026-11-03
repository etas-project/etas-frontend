module method_resolution.closed_owner;

spec Allowed;
impl string ~ Allowed;

type Container<T ~ Allowed> = { value: T };

impl Container<string> {
    public flow copy(self) -> Container<string> {
        return self;
    }

    public flow choose<T>(self, input: T) -> T {
        return input;
    }
}

flow main() -> i32 {
    return Container<string> { value = "closed" }.copy().choose<i32>(8);
}
