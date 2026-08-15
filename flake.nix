{
  description = "A modern highly-customizable Subsonic TUI client built in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        cargoToml = builtins.fromTOML (builtins.readFile ./ratune/Cargo.toml);

        ratune = pkgs.rustPlatform.buildRustPackage {
          pname = "ratune";
          version = cargoToml.package.version;

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            pkgs.alsa-lib
            pkgs.dbus
            pkgs.openssl
          ];

          meta = with pkgs.lib; {
            description = "A modern highly-customizable Subsonic TUI client built in Rust";
            homepage = "https://github.com/acmagn/ratune";
            license = licenses.mit;
            mainProgram = "ratune";
          };
        };
      in
      {
        packages.default = ratune;
        packages.ratune = ratune;

        apps.default = {
          type = "app";
          program = "${ratune}/bin/ratune";
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ ratune ];
          nativeBuildInputs = [
            pkgs.rustc
            pkgs.cargo
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      }
    )) // {
      overlays.default = final: prev: {
        ratune = self.packages.${prev.system}.default;
      };

      homeManagerModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.ratune;
          tomlFormat = pkgs.formats.toml { };

          finalSettings = lib.recursiveUpdate
            (lib.optionalAttrs (cfg.server.url != null || cfg.server.username != null) {
              server = lib.filterAttrs (_: v: v != null) {
                url = cfg.server.url;
                username = cfg.server.username;
              };
            })
            cfg.settings;
        in
        {
          options.programs.ratune = {
            enable = lib.mkEnableOption "Ratune Subsonic TUI music player";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.default;
              description = "The ratune package to install.";
            };

            server = {
              url = lib.mkOption {
                type = lib.types.nullOr lib.types.str;
                default = null;
                example = "https://navidrome.example.com";
                description = "URL of the Subsonic-compatible server.";
              };

              username = lib.mkOption {
                type = lib.types.nullOr lib.types.str;
                default = null;
                example = "alice";
                description = "Username for the Subsonic-compatible server.";
              };
            };

            settings = lib.mkOption {
              type = tomlFormat.type;
              default = { };
              example = lib.literalExpression ''
                {
                  theme.preset = "dynamic";
                  player.default_volume = 80;
                }
              '';
              description = "Additional arbitrary configuration written to ~/.config/ratune/config.toml";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];

            xdg.configFile."ratune/config.toml" = lib.mkIf (finalSettings != { }) {
              source = tomlFormat.generate "ratune-config.toml" finalSettings;
            };
          };
        };
    };
}
