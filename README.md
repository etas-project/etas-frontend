# Etas Frontend

Parses Etas source, checks types and effects, and produces checked HIR.

HIR is the structured program representation shared with downstream tools.
Checked HIR carries the resolved program and type/effect facts needed by the
interpreter, so execution does not have to repeat language checking.

The frontend handles parsing, module/name resolution, lowering, type checking,
and effect inference. It does not download dependencies or execute programs;
the CLI supplies the project and prepared package environment.

Experimental. To write and run Etas programs, start with the
[Etas CLI](https://github.com/etas-project/etas#quickstart).

## Bootstrap

Requires Rust 1.85+ and a native C/C++ build toolchain.

```sh
git clone https://github.com/etas-project/etas-frontend.git
cd etas-frontend
cargo build --workspace
```

This builds the frontend libraries. To start with the lexer/parser tests:

```sh
cargo test -p etas_syntax
```

## Documentation

- [Frontend design](docs/architect/phase1-frontend-design.md)
- [Language specification](https://github.com/etas-project/etas/tree/main/docs/design)

[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE)

The source standard-library implementation and bootstrap contracts are documented
in [Etas Core](https://github.com/etas-project/etas-core/blob/main/docs/std-source-declarations.md).
