module library.api;

public spec Encode {
    flow encode(self) -> i32 ![];
}

public type Packet = { value: i32 };
public type Encoded = { value: i32 };

agent Hidden() -> unit { return; }
type PrivatePacket = { value: i32 };
impl PrivatePacket { public flow value(self) -> i32 { return self.value; } }

impl Packet {
    public flow new(value: i32) -> Packet ![] {
        return Packet { value };
    }

    public flow copy(source: Packet) -> Packet ![] {
        return Packet { value = source.value };
    }

    public flow convert(self, increment: i32) -> Encoded ![] {
        return Encoded { value = self.value + increment };
    }

    flow private_value(self) -> i32 ![] { return self.value; }
}

impl Encoded ~ Encode {
    flow encode(self) -> i32 ![] { return self.value; }
}

flow main() -> i32 ![] {
    let original = Packet.new(7);
    return Packet.copy(original).convert(3)::Encode.encode();
}
