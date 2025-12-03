{
  description = "Niri: A scrollable-tiling Wayland compositor.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    fenix.url = "github:nix-community/fenix";

    treefmt-nix.url = "github:numtide/treefmt-nix";

    crane.url = "github:ipetkov/crane";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      treefmt-nix,
      fenix,
      crane,
      advisory-db,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      inherit (nixpkgs) lib;

      forEachSupportedSystem =
        f:
        lib.genAttrs supportedSystems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [
                self.overlays.default
              ];
            };

            ourPackages = lib.filterAttrs (_: v: (v ? niriPackage)) pkgs.niriPackages;

            treefmtEval = treefmt-nix.lib.evalModule pkgs ./treefmt.nix;

            treefmt = treefmtEval.config.build.wrapper;
          in
          f {
            inherit
              crane
              fenix
              ourPackages
              pkgs
              system
              treefmt
              treefmtEval
              ;
          }
        );
    in
    {
      formatter = forEachSupportedSystem ({ treefmt, ... }: treefmt);

      checks = forEachSupportedSystem (
        {
          pkgs,
          treefmtEval,
          ourPackages,
          ...
        }:
        let
          testsFrom =
            pkg:
            pkgs.lib.mapAttrs' (name: value: {
              name = "${pkg.pname}-${name}";
              inherit value;
            }) (pkg.passthru.tests or { });

          ourTests = pkgs.lib.foldlAttrs (
            acc: name: value:
            acc // (testsFrom value)
          ) { } ourPackages;
        in
        ourTests
        // {
          treefmt = treefmtEval.config.build.check self;

          # build tests
          build-niri = pkgs.niriPackages.niri;
          build-niri-visual-tests = pkgs.niriPackages.niri-visual-tests;
        }
      );

      devShells = forEachSupportedSystem (
        {
          pkgs,
          ourPackages,
          treefmt,
          ...
        }:
        let
          ourBuildInputs = lib.unique (
            lib.foldlAttrs (
              acc: _: v:
              acc ++ (v.buildInputs or [ ]) ++ (v.nativeBuildInputs or [ ])
            ) [ ] ourPackages
          );
        in
        {
          default = pkgs.mkShell {
            inputsFrom = builtins.attrValues ourPackages;

            packages = [
              pkgs.rustPlatform.bindgenHook
              pkgs.pkg-config
              pkgs.wrapGAppsHook4 # For `niri-visual-tests`
              pkgs.cargo-insta
              treefmt
            ];

            buildInputs = [
              pkgs.libadwaita # For `niri-visual-tests`
            ];

            env = {
              LD_LIBRARY_PATH = builtins.concatStringsSep ":" (
                map (e: "${e.lib or e.out}/lib") (
                  ourBuildInputs
                  ++ [
                    pkgs.glib
                    pkgs.pixman

                    # for `niri-visual-tests`
                    pkgs.libadwaita
                    pkgs.gtk4
                  ]
                )
              );
            };
          };
        }
      );

      packages = forEachSupportedSystem ({ ourPackages, ... }: ourPackages);

      nixosModules.default = import ./nix/modules/niri-nixos.nix { overlay = self.overlays.default; };

      homeManagerModules.default = import ./nix/modules/niri-home-manager.nix;

      overlays.default = final: _: {
        niriPackages = final.callPackage ./scope.nix {
          inherit
            advisory-db
            crane
            fenix
            self
            ;
        };
      };
    };
}
