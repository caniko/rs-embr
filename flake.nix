{
  description = "embr — declarative project code embedding indexer";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    self,
    nixpkgs,
    flake-parts,
    crane,
    rust-overlay,
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux" "aarch64-linux"];

      perSystem = {
        system,
        pkgs,
        ...
      }: let
        pkgsWithRust = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };
        rustToolchain =
          pkgsWithRust.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        craneLib = (crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

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

        embr = craneLib.buildPackage (commonArgs
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
          });
      in {
        _module.args.pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };

        packages = {
          inherit embr;
          default = embr;
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
