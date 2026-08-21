# Conformance — xyz-rust

Specification target: [xyz-spec](https://github.com/ejfkdev/xyz-spec) **v0.1.1**.

Status: **conformant (baseline anchor)** — xyz-rust is one of the two
reference implementations the specification was written from.

Deviations register: see
[deviations.md](https://github.com/ejfkdev/xyz-spec/blob/main/deviations.md),
entries `D-rust-01` … `D-rust-10`.

## Checklist

Every Class A item (conformance.md) is implemented and covered by the
following evidence:

| Evidence | Covers |
|---|---|
| `cargo test -p xyz-rust --lib` (73 tests: errors/logx/registry/spec/cli/dispatch/httpapi/mcp) | A.1–A.42 pipeline, taxonomy, rendering, dispatcher semantics |
| `.github/workflows/test.yml` — six combination matrix (`default`, no-mcp, no-cli, no-http, cli-only, embedding-only) + fmt/clippy + MSRV 1.88 | A.38–A.39 trim invariants |
| `examples/example` (11 commands), `examples/tour`, `examples/clap` | showcase fixture §3.1, invocation matrix §3.2 |
| `docs/adapters.md` | A.41 embedding surfaces, §15.2 documentation |

Golden outputs (conformance.md §3.3) were diff-verified byte-for-byte
against the Rust binary — and the byte-exact segments (`file hash`, `math
sum`, `math div`, `/healthz`) additionally cross-checked against xyz-go
(identical bytes across SDKs).

## Showcase evidence

The fixture program runs with
`cargo run -p xyz-example -- <command>`; all commands of the §3.2 matrix
behave as specified, including `search query --query golang` (CLI default
k=25), the SHA-256 `file hash` answer, the rejection of `mcp sse`
(codified by spec §12.3 — see D-rust-06), and the default-subcommand
forwarding of §10.1 (`cli::cli_test::default_subcommand_forwards_all_args`).

## Deviations

All deviations are filed in the spec repository's
[deviations.md](https://github.com/ejfkdev/xyz-spec/blob/main/deviations.md)
with section references and classes (language-forced / SDK limitation /
extension); this document only points there to keep a single source of
truth.