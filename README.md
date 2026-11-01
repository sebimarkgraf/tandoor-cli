# tandoor-cli

An async Rust SDK and a CLI for [Tandoor Recipes](https://docs.tandoor.dev/).
The library owns HTTP, authentication, models, pagination, and API errors. The
binary adds argument parsing, environment configuration, input, and presentation.

## Install and configure

```sh
cargo install --path . --locked
export TANDOOR_URL='https://recipes.example.com'
export TANDOOR_TOKEN='your-api-token'
tandoor list
```

For development, use `cargo run -- <arguments>` in place of `tandoor`.
The URL can be an instance root, an `/api/` root, or include a deployment subpath
(for example, `https://example.com/recipes/`).

Authentication is explicit; the client does not attempt alternative authentication
schemes after a failure:

| `TANDOOR_AUTH` / `--auth` | Credential in `TANDOOR_TOKEN` | Request header |
| --- | --- | --- |
| `bearer` (default) | OAuth API token | `Authorization: Bearer …` |
| `session` | Allauth session token | `X-Session-Token: …` |
| `legacy-token` | DRF token on older instances | `Authorization: Token …` |

Tandoor's [authentication documentation](https://docs.tandoor.dev/features/authentication/#3rd-party-clients)
describes obtaining an allauth session token and recommends session authentication
for third-party clients. Set `TANDOOR_AUTH=session` when using one. API token
availability and permissions depend on your server version and account.
This CLI accepts an existing credential; it does not implement interactive login.

`--url`, `--token`, and `--auth` override their environment variables. Credentials
are not saved. `.env` is ignored by Git but is **not** automatically loaded.
The request timeout is 30 seconds; override it with `--timeout SECONDS`.

## Recipes

```sh
tandoor list --query 'pasta' --page-size 10
tandoor list --all --sort name
tandoor get 42
tandoor create --name 'Soup' --servings 2 --instruction 'Simmer for 20 minutes.'
tandoor create --file examples/recipe.json
printf '%s\n' '{"description":"Weeknight dinner","servings":4}' |
  tandoor update 42 --file -
tandoor delete 42 --yes
```

`search` aliases `list`. Use `--help`
on any command to discover its arguments. Commands do not prompt, and deletion
requires an explicit `--yes`.

Creation supports simple flags or a JSON object from a file or stdin (`--file -`).
See [`examples/recipe.json`](examples/recipe.json) for steps, ingredients, and
keywords. New recipes default to `internal: true`, one serving, and no steps.
Food and unit references accept existing numeric IDs or nested named objects;
keyword references can be supplied in the JSON body as IDs or named objects.

Updates use HTTP PATCH. Omitted fields remain unchanged; an empty array is an
explicit change. Tandoor controls nested-array replacement behavior: when editing
steps, ingredients, or keywords, fetch the recipe first, retain existing IDs, and
send the complete intended array. Fields such as `description` can be set to JSON
`null`. Read responses and write payloads are separate types; avoid blindly
posting an entire response, which includes server-managed fields.

## Agent and scripting contract

```sh
tandoor --output json list --query 'pasta'
tandoor --output json get 42 | jq '.steps[].ingredients'
cat examples/recipe.json | tandoor --output json create --file -
```

- `--output auto` (default) prints readable text when stdout is a terminal and
  JSON when stdout is redirected or piped. `--output json` and `--output text`
  select a format explicitly. JSON is emitted as one complete document per command.
- Success data goes to stdout. Runtime errors go to stderr, leaving stdout empty.
  In JSON mode errors have the form
  `{"error":{"kind":"api","message":"…","status":404,"details":{"detail":"Not found."}}}`.
  Other kinds are `input`, `configuration`, `transport`, `json`, and `pagination`.
  `status` and `details` are present only for API errors.
- Exit codes: `0` success, `1` runtime failure, `2` argument/usage error.
  Clap's help, version, and usage errors are text even when JSON is selected.
- `list` returns `{count,next,previous,results}`. `--all` follows pagination
  and returns an array, starting from `--page` if supplied. Detail, create, and
  update commands return a recipe object; lists omit `steps`.
- Delete returns `{"deleted":true,"id":42}`. Generic API commands return the
  server's JSON; an empty successful response becomes `null`.
- Unknown server fields are retained. This is a normalized SDK representation;
  fields with local defaults may be added. Use `api` for unmodified JSON values.

## Other API resources

Use the generic API command for endpoints beyond the typed recipe surface:

```sh
tandoor api keyword/ --query query=Dinner
tandoor api meal-plan/ --query from_date=2026-11-01 --query to_date=2026-11-07
printf '%s\n' '{"name":"Dinner"}' |
  tandoor api keyword/ --method POST --file - --yes
```

Paths are relative to the configured `/api/` root. Repeat `--query key=value`
for multiple parameters. Write methods (`POST`, `PUT`, `PATCH`, `DELETE`) require
`--yes`. Endpoint availability and payloads depend on the server. Current Tandoor
exposes its OpenAPI schema at `/openapi/` and API documentation at `/docs/api/`;
consult your instance for its exact API.
Redirects are disabled, and authenticated requests and pagination are restricted
to the configured API root.

## SDK

The package is `tandoor-cli`; the library crate is `tandoor`. A local consumer can
rename the package dependency:

```toml
[dependencies]
tandoor = { package = "tandoor-cli", path = "../tandoor-cli" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust,no_run
use tandoor::{Auth, Client, NewRecipe, RecipePatch, RecipeQuery};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new(
        &std::env::var("TANDOOR_URL")?,
        Auth::Bearer(std::env::var("TANDOOR_TOKEN")?),
    )?;

    let recipes = client.list_all_recipes(&RecipeQuery {
        query: Some("pasta".into()),
        ..Default::default()
    }).await?;
    for recipe in recipes {
        println!("{}: {}", recipe.id, recipe.name);
    }

    let created = client.create_recipe(&NewRecipe::new("Soup")).await?;
    let patch = RecipePatch { servings: Some(4), ..Default::default() };
    let updated = client.update_recipe(created.id, &patch).await?;
    println!("{}", updated.name);
    Ok(())
}
```

`Client` is cloneable and reuses its HTTP connection pool. It does not read the
environment or write to stdout/stderr. `Client::with_timeout` configures timeouts,
`next_page` fetches one additional page, and `request::<T>` allows any
deserializable response type with `tandoor::Method`, relative paths, query pairs,
and an optional `serde_json::Value` body. `Error::Api { status, body }` preserves
HTTP failures and validation details for callers. `cargo doc --open` shows the
public API.

## Development and compatibility

With Nix (flakes enabled):

```sh
nix develop          # Rust toolchain, Clippy, rustfmt, rust-analyzer
nix flake check -L   # Format, lint, test, and build
nix build           # CLI at ./result/bin/tandoor
nix run -- --help
```

The pinned flake supports Linux on x86-64 and ARM64, and macOS on ARM64. Linux also
provides `nix build .#release`, a tarball containing a static CLI binary that
runs without Nix.

GitHub Actions runs the flake checks on pull requests, pushes to `main`, and
`v*` tags. To publish, add a crates.io API token as the repository secret
`CARGO_REGISTRY_TOKEN`, update `Cargo.toml` and `Cargo.lock`, and push a tag
matching the package version (for example, `v0.1.0`). After checks pass, the
workflow publishes to crates.io and creates a GitHub Release with a Linux
x86-64 binary. The GitHub token is supplied automatically.

Publishing is implemented in the flake and can also run from a clean Linux
checkout with `CARGO_REGISTRY_TOKEN` and `GH_TOKEN` set:

```sh
nix run .#publish -- v0.1.0 --dry-run # Validate packaging without publishing
nix run .#publish -- v0.1.0           # Publish to crates.io and GitHub Releases
```

Without Nix:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Tests use a local mock HTTP server and cover request methods, credentials,
pagination, URL restrictions, errors, timeouts, nested JSON, and CLI behavior.
They do not require an account. No live Tandoor instance has been tested yet.

The initial implementation follows the official
[API routes](https://github.com/TandoorRecipes/recipes/blob/cbb96e151fe0c57c16a4456fe1bd04230b1c1e5c/cookbook/urls.py),
[serializers](https://github.com/TandoorRecipes/recipes/blob/cbb96e151fe0c57c16a4456fe1bd04230b1c1e5c/cookbook/serializer.py),
and [authentication settings](https://github.com/TandoorRecipes/recipes/blob/cbb96e151fe0c57c16a4456fe1bd04230b1c1e5c/recipes/settings.py).
Typed coverage is intentionally limited to recipe CRUD/search; other Tandoor
features use the generic API surface. Legacy authentication is selectable, but
this is not a promise of compatibility with every historical server release.

## License

Licensed under AGPLv3. See [LICENSE.md](LICENSE.md) for the full terms.
