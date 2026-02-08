{
  description = "Niri: A scrollable-tiling Wayland compositor.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    # FIXME: remove once https://github.com/NixOS/nixpkgs/pull/476455 is merged
    nixpkgs-tracy.url = "github:davidkern/nixpkgs?ref=tracy-split-package";

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
      nixpkgs-tracy,
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
                (
                  final: prev:
                  let
                    pkgs-tracy = import nixpkgs-tracy {
                      inherit (final.stdenv.hostPlatform) system;
                    };
                  in
                  {
                    inherit (pkgs-tracy) tracy;
                  }
                )
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

            packages =
              let
                # we need to use `addr2line` from `llvmPackages` instead of `binutils`, hence the override
                perf = pkgs.perf.override {
                  binutils-unwrapped = pkgs.llvmPackages.bintools-unwrapped;
                };

                cargo-flamegraph = pkgs.cargo-flamegraph.override {
                  inherit perf;
                };
              in
              [
                pkgs.tracy
                pkgs.cargo-insta
                pkgs.flamegraph
                pkgs.pkg-config
                pkgs.rustPlatform.bindgenHook
                pkgs.wrapGAppsHook4 # For `niri-visual-tests`

                cargo-flamegraph
                perf
                treefmt
              ];

            buildInputs = [
              pkgs.libadwaita # For `niri-visual-tests`
            ];

            env = {
              # to make niri load `config-debug.kdl` even when built with release profile
              NIRI_DEV = "true";
              LD_LIBRARY_PATH = builtins.concatStringsSep ":" (
                map (e: "${e.lib or e.out}/lib") (
                  ourBuildInputs
                  ++ [
                    pkgs.stdenv.cc.cc

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
