# embr

<!-- simit:badges:start -->
![CI](https://img.shields.io/badge/CI-managed-2088ff) [![Nix](https://img.shields.io/badge/Nix-managed-5277c3)](flake.nix) [![crates.io](https://img.shields.io/badge/crates.io-ready-f46623)](https://crates.io/crates/embr-cli)
<!-- simit:badges:end -->

Declarative project code embedding indexer. Walks a list of projects,
chunks source files (line-window for v1, tree-sitter planned), embeds
each chunk with one or more ollama models, and upserts them into qdrant
as named vectors. Incremental on SHA-256 of file content — re-runs only
touch changed files — with per-project orphan cleanup on deletion.

## Status

v1, pre-tag. Line-based chunking; ollama + qdrant only; watch mode is
pure polling (uniform behaviour on local and NFS).

## Install / run

```
nix run github:caniko/rs-embr -- index --config /path/to/embr.toml
```

The CLI accepts:

- `embr index` — one full pass, exit
- `embr watch` — long-running, re-indexes projects when they change
- `embr status` — per-project file counts from the local state DB
- `embr reset` — delete local state DB; does not touch qdrant

## Config

```toml
projects_root = "/mnt/atlas-projects"
projects = ["canix", "nix-infernis", "rs-embr"]

[qdrant]
url = "http://localhost:6333"
collection = "projects"

[embedding]
url = "http://localhost:11434"
backend = "ollama"

[[embedding.vectors]]
name = "text"
model = "nomic-embed-text"
dim = 768

[[embedding.vectors]]
name = "code"
model = "nomic-embed-code"
dim = 768

[chunking]
max_lines = 80
overlap = 10
# extensions + max_file_bytes have sensible defaults

[watch]
interval_secs = 30
```

All named vectors must currently share the same dimension. The pipeline
creates the qdrant collection on first run if absent.

For an OpenAI-compatible local server such as llama-swap, set the backend to
openai and point the URL at its base URL, for example
http://127.0.0.1:8013/v1. No API key is required for the local deployment.

## NixOS module

`nixosModules.default` exposes `services.embr.*`. Minimal example:

```nix
{
  imports = [ inputs.embr.nixosModules.default ];

  services.embr = {
    enable = true;
    package = inputs.embr.packages.${pkgs.system}.embr;
    projectsRoot = "/mnt/atlas-projects";
    projects = [ "canix" "nix-infernis" ];
    embedding.vectors = [
      { name = "text"; model = "nomic-embed-text"; dim = 768; }
      { name = "code"; model = "nomic-embed-code"; dim = 768; }
    ];
    requires = [ "qdrant.service" "ollama.service" ];
  };
}
```

The module generates a TOML config, runs `embr watch` as a dedicated
system user, and hardens the systemd unit (`ProtectSystem=strict`,
`ReadOnlyPaths=[projectsRoot]`, etc.).

## CI

Woodpecker CI on Codeberg runs `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt --check` on every push and pull request.

## License

Dual-licensed under MIT or Apache-2.0 at your option.
