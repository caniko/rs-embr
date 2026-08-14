{
  description = "embr — declarative project code embedding indexer";

  inputs = {
    rs-harbor.url = "git+ssh://git@codeberg.org/caniko/rs-harbor.git?ref=trunk&rev=f209ddbca3fdbb0dc31fa3886ccc2ff7369c18ac";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    plinth = {
      url = "git+https://codeberg.org/caniko/plinth.git?ref=refs/heads/trunk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    self,
    rs-harbor,
    nixpkgs,
    flake-parts,
    crane,
    rust-overlay,
    plinth,
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      # rs-embr (the embr code-embedding indexer) runs only on x86_64
      # AI hosts via infernix; no aarch64-linux consumer exists, so
      # evaluating aarch64 outputs is dead weight that doubles
      # `nix flake check` heap for nothing.
      systems = ["x86_64-linux"];

      perSystem = {
        system,
        pkgs,
        ...
      }: let
        pkgsWithRust = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };
        toolchain = rs-harbor.lib.mkToolchain { inherit pkgs; toolchainProfile = "stable"; };

        craneLib = toolchain.craneLib;
        buildCache = rs-harbor.lib.mkBuildCachePolicy {
          inherit pkgs;
          sccachePackage = rs-harbor.packages.${system}.sccache;
          cacheRoot = null;
          namespaceScope = "canix-rust";
          namespaceGeneration = 5;
        };

        # Include the Cargo workspace and nothing else (no nix/, no docs,
        # no target/) so caching is stable.
        src = craneLib.cleanCargoSource ./.;

        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "embr";
          version = "0.1.0";

          nativeBuildInputs = [pkgs.pkg-config];
          buildInputs = [pkgs.openssl];
        };

        # Cache compiled deps separately from workspace sources.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        embr = buildCache.withRustCache { package = craneLib.buildPackage (commonArgs
          // {
            inherit cargoArtifacts;
            cargoExtraArgs = "-p embr-cli";
            meta.mainProgram = "embr";
            doCheck = true;

            # Ensure `git` is available at runtime — the walker shells out
            # to it and we can't rely on the system path.
            nativeBuildInputs = (commonArgs.nativeBuildInputs or []) ++ [pkgs.makeWrapper];
            postInstall = ''
              wrapProgram $out/bin/embr \
                --prefix PATH : ${pkgs.lib.makeBinPath [pkgs.git]}
            '';
          }); };
        website = plinth.lib.${system}.mkProjectSite {
          pname = "embr-website";
          domain = "embr.tartanoglu.com";
          configPath = ./website/plinth-project.toml;
        };
      in {
        _module.args.pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };

        packages = {
          inherit embr;
          default = embr;
          website = website;
          site = website;
        };

        apps.deploy-pages = plinth.lib.${system}.mkDeployPagesApp {
          domain = "embr.tartanoglu.com";
        };

        checks = {
          inherit embr;
          embr-clippy = craneLib.cargoClippy (commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            });
          embr-fmt = craneLib.cargoFmt {inherit src;};
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgs; [
            git
            pkg-config
            openssl
            rust-analyzer
          ];
        };
      };

      flake = {
        nixosModules = {
          default = import ./nix/module.nix;
        };
      };
    };
}
