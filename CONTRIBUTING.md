# Working on Rift Event Server

This is Nissoco's server-only fork of [Horizon](https://github.com/horizon-rust/horizon). How to build and run
it is in [README.md](README.md); where things live is in [AGENTS.md](AGENTS.md) and
[docs/architecture.md](docs/architecture.md).

## Rules

- **Never commit Mojang's files:** game jars, assets, decompiled source or the prepared game files (`.local/`).
  `tools/check_public_tree.py` rejects them, and `git config core.hooksPath .githooks` runs it on every commit.
- **Vanilla is the specification** wherever it has the feature. The harnesses in `tools/` compare the engine
  with the real Java game, so do not delete them or loosen what they pin. Read [the workflow](docs/parity-workflow.md)
  before changing vanilla behaviour.
- **Names:** text a player or an operator reads says Rift; internal names stay Horizon's
  ([docs/rift-naming.md](docs/rift-naming.md)).
- **Tests check behaviour:** against an independent expected value or a vanilla oracle, not against words in
  docs or comments.
- **Keep changes small** and keep the build green: `scripts/build.sh` and the tests of the crates you touched.

```sh
cargo test --profile fast -p horizon-server -p horizon-server-files -p horizon-anticheat -p horizon-chat-filter
cargo fmt --all --check
```

## Licence

The code is [MIT](LICENSE). A contribution is made under the same licence. Third-party code keeps its own
([THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)).
