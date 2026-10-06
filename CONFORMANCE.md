# Conformance — xyz-rust

Specification target: [xyz-spec](https://github.com/ejfkdev/xyz-spec) **v0.4.4**.

Status: **conformant (baseline anchor)** — xyz-rust is one of the two
reference implementations the specification was written from.

Deviations register: see
[deviations.md](https://github.com/ejfkdev/xyz-spec/blob/main/deviations.md),
entries `D-rust-01` … `D-rust-13`.

## Checklist

Every Class A item (conformance.md) is implemented and covered by the
following evidence:

| Evidence | Covers |
|---|---|
| `cargo test -p xyz-rust --lib` (112 tests: errors/logx/registry/spec/cli/dispatch/httpapi/mcp) | A.1–A.53 pipeline, taxonomy, rendering, dispatcher semantics, rich errors, formats, headers, unions, blocks |
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
forwarding of §10.1 (`cli::cli_test::default_subcommand_forwards_all_args`),
plus the custom help blocks of §10.4/§13.2
(`dispatch_test::overview_help_blocks`, `cli::cli_test::help_blocks_on_leaf_only`)
and the language catalog of §15.5 (`lang::tests`,
`dispatch_test::lang_resolution_and_catalog`), plus §4.5a/§13.9/§6.1/
§10.4 (`cli::cli_test::channel_skip_and_help_types`,
`cli::cli_test::daemon_style_command`, `dispatch_test::try_run_composability`,
`dispatch_test::channel_defaults_flag`).

## v0.3.x → v0.4.2 additions

The clauses landed between the v0.3.0 anchor and v0.4.2, each with the
evidence that locks it:

- **§4.5a/§6.1/§13.9 (gs needs, v0.3.1–v0.3.2)** — channel skips, daemon
  marker, `--default`, bare-flag passthrough, `try_run`:
  `cli::cli_test::channel_skip_and_help_types`,
  `cli::cli_test::daemon_style_command`, `dispatch_test::try_run_composability`,
  `dispatch_test::channel_defaults_flag`.
- **§12.4a MCP tool-name override (v0.3.3)** — `MCPHints.name`, grammar-checked
  at registration: `mcp::mcp_test::mcp_name_override`.
- **§4.7 tagged unions (v0.4.0)** — adjacent-tag enums → `oneOf` with `const`
  discriminators, exactly-one-branch decode, per-frontend skip policy:
  `spec::spec_test::tagged_union_schema_and_decode`,
  `spec::spec_test::tagged_union_variant_rename_matches_serde`,
  `cli::cli_test::union_field_degrades_not_fatals`.
- **§12.7 content-block results (v0.4.0)** — reserved `content` envelope,
  MCP verbatim blocks, CLI temp-file projection:
  `blocks::tests`, `mcp::mcp_test::block_envelope_passes_through`,
  `cli::cli_test::block_envelope_spills_binary_to_files`.
- **§8.5/§8.6 rich errors (v0.4.2)** — `with_code/with_detail/with_status`
  builders, shared error body on HTTP/CLI-machine/MCP:
  `errors` unit tests, `httpapi::httpapi_test::rich_error_body_and_status_override`,
  `mcp::mcp_test::result_meta_shape`.
- **§10.7 `--format` (v0.4.2)** — text/json/jsonl/markdown, `--json` alias,
  bare-flag yield rule, `--xyz.format`:
  `cli::format::tests`, `cli::cli_test::format_dispatch_and_conflict_yield`,
  `cli::cli_test::format_flag_yields_to_command_field`.
- **§11.6 server-context headers (v0.4.2)** —
  `httpapi::httpapi_test::server_context_headers_on_all_responses`; app
  identity vs SDK version split is in `version` module docs and README §差异.
- **§12.6/§12.8 MCP identity & result `_meta.xyz` (v0.4.2)** —
  `mcp::mcp_test::result_meta_shape`.

## v0.4.4 additions

- **§10.7 TTY-aware `auto` + five-tier precedence** — `Format::Auto`
  resolves by the output writer (forceable via `cli::Options.interactive`;
  injected writers are non-interactive); tiers: bare flag > `--xyz.format`
  > `CliHints.format` > `Config.Format` > `auto`; halves configurable via
  `Config.FormatInteractive/FormatPiped`. Evidence:
  `cli::cli_test::format_auto_tty_resolution`,
  `cli::format::tests::format_parsing`.
- **§10.7a format/style axes** — shared TTY probe is `termx`
  (`Interactive`/`NoColor`), surfaced as `xyz_rust::{interactive, no_color}`;
  the style axis stays reserved (format unaffected by NO_COLOR).
- **§11.7 per-request language** — `Accept-Language` resolved per request
  (`lang::parse_accept_language`, q-ordered), carried on the request `Ctx`
  (`Ctx::with_language`), read by handlers via
  `xyz_rust::language_from_ctx`, and used to localize framework messages
  (`http.err_invalid_json`). Evidence:
  `httpapi::httpapi_test::accept_language_localizes_and_reaches_handler`.
- **§13.1 four modes + `xyz.<word>` + shadowing** — `serve`/`http` (REST
  only, no `/mcp`)/`mcp`/`help`; `xyz.<word>` always reaches the built-in;
  a user command whose top segment equals a mode word shadows the bare
  form (no more registration error); CLI-skipped commands do not shadow;
  the overview lists only unshadowed words. Evidence:
  `dispatch_test::shadowing_modes_and_namespaced_reachability`,
  `dispatch_test::cli_skipped_commands_do_not_shadow`,
  `dispatch_test::mode_words_are_no_longer_reserved`.
- **§10.4/§13.2 help subcommand & mode `-h`** — `help` → overview;
  `help <mode>` → mode help; `help <command-path>` (dotted or spaced) →
  detailed command help; `serve|http|mcp -h` prints mode help without
  starting. Evidence: `dispatch_test::help_subcommand_family`,
  `dispatch_test::mode_help_does_not_start_servers`.
- **§14 item 7 environment context** — `xyz_rust::{language, interactive,
  no_color, env()}` + `EnvContext` + `language_from_ctx`. Evidence:
  `cli::cli_test::ctx_language_and_env_api`.

Known remaining gaps are in the deviations register: D-rust-12 closed
(MCPHints.name), D-go-01 (tagged unions in Go) is the Go side's open item;
the reserved style axis (§10.7a colour) is explicitly future work per spec.

## Deviations

All deviations are filed in the spec repository's
[deviations.md](https://github.com/ejfkdev/xyz-spec/blob/main/deviations.md)
with section references and classes (language-forced / SDK limitation /
extension); this document only points there to keep a single source of
truth.