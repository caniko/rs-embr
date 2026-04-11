# embr NixOS module.
#
# Exposes `services.embr.*`. The module generates a TOML config that
# matches embr-core's Config schema, wires it into a systemd service
# running `embr watch`, and grants the service user read access to the
# configured projects root.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.embr;

  namedVectorSubmodule = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        example = "text";
        description = "Qdrant vector name.";
      };
      model = lib.mkOption {
        type = lib.types.str;
        example = "nomic-embed-text";
        description = "Ollama model identifier to embed with.";
      };
      dim = lib.mkOption {
        type = lib.types.ints.positive;
        example = 768;
        description = "Dimension of the vector. Must match the model's output.";
      };
    };
  };

  configToml = (pkgs.formats.toml {}).generate "embr.toml" {
    qdrant = {
      url = cfg.qdrant.url;
      collection = cfg.qdrant.collection;
    };
    embedding = {
      url = cfg.embedding.url;
      vectors = cfg.embedding.vectors;
    };
    projects_root = toString cfg.projectsRoot;
    projects = cfg.projects;
    chunking = {
      max_lines = cfg.chunking.maxLines;
      overlap = cfg.chunking.overlap;
      extensions = cfg.chunking.extensions;
      max_file_bytes = cfg.chunking.maxFileBytes;
    };
    watch = {
      interval_secs = cfg.watch.intervalSecs;
      force_poll = cfg.watch.forcePoll;
    };
  };
in {
  options.services.embr = {
    enable = lib.mkEnableOption "embr — declarative project code embedding indexer";

    package = lib.mkOption {
      type = lib.types.package;
      description = "embr package to use. Set by downstream flakes via an overlay or direct assignment.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "embr";
      description = "System user the indexer runs as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "embr";
      description = "Primary group for the indexer user.";
    };

    projectsRoot = lib.mkOption {
      type = lib.types.path;
      example = "/mnt/atlas-projects";
      description = "Base directory containing project sub-directories.";
    };

    projects = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      example = ["canix" "nix-infernis"];
      description = "Sub-directory names under projectsRoot to index.";
    };

    qdrant = {
      url = lib.mkOption {
        type = lib.types.str;
        default = "http://localhost:6333";
        description = "Qdrant base URL.";
      };
      collection = lib.mkOption {
        type = lib.types.str;
        default = "projects";
        description = "Qdrant collection name. Created on first run if absent.";
      };
    };

    embedding = {
      url = lib.mkOption {
        type = lib.types.str;
        default = "http://localhost:11434";
        description = "Ollama base URL.";
      };
      vectors = lib.mkOption {
        type = lib.types.listOf namedVectorSubmodule;
        example = [
          {
            name = "text";
            model = "nomic-embed-text";
            dim = 768;
          }
          {
            name = "code";
            model = "nomic-embed-code";
            dim = 768;
          }
        ];
        description = ''
          Named vectors to produce for each chunk. All dimensions must match
          (embr enforces this during config validation).
        '';
      };
    };

    chunking = {
      maxLines = lib.mkOption {
        type = lib.types.ints.positive;
        default = 80;
        description = "Maximum lines per chunk.";
      };
      overlap = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 10;
        description = "Overlap between adjacent chunks, in lines. Must be < maxLines.";
      };
      extensions = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [
          "nix"
          "rs"
          "ts"
          "tsx"
          "js"
          "jsx"
          "py"
          "go"
          "md"
          "txt"
          "toml"
          "yaml"
          "yml"
          "json"
          "sh"
          "nu"
          "html"
          "css"
          "scss"
          "sql"
          "proto"
          "graphql"
          "lua"
          "c"
          "h"
          "cpp"
          "hpp"
        ];
        description = "File extensions considered source code.";
      };
      maxFileBytes = lib.mkOption {
        type = lib.types.ints.positive;
        default = 500000;
        description = "Skip files larger than this (bytes).";
      };
    };

    watch = {
      intervalSecs = lib.mkOption {
        type = lib.types.ints.positive;
        default = 30;
        description = "Watch poll interval in seconds.";
      };
      forcePoll = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Force polling mode even on local filesystems (for debugging).";
      };
    };

    requires = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      example = ["qdrant.service" "ollama.service"];
      description = ''
        Extra systemd units to order after. Use this when qdrant and/or
        ollama run on the same host so embr waits for them to be up.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "embr indexer";
    };
    users.groups.${cfg.group} = {};

    systemd.services.embr = {
      description = "embr — project code embedding indexer";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"] ++ cfg.requires;
      wants = ["network-online.target"];
      requires = cfg.requires;

      environment = {
        EMBR_CONFIG = toString configToml;
        RUST_LOG = "info,embr=debug";
      };

      serviceConfig = {
        Type = "simple";
        ExecStart = "${lib.getExe cfg.package} watch";
        Restart = "on-failure";
        RestartSec = 10;
        User = cfg.user;
        Group = cfg.group;
        StateDirectory = "embr";
        StateDirectoryMode = "0750";

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        SystemCallArchitectures = "native";
        ReadOnlyPaths = [(toString cfg.projectsRoot)];
      };
    };
  };
}
