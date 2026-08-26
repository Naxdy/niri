{
  advisory-db,
  autoPatchelfHook,
  cairo,
  craneLib,
  dbus,
  glib,
  installShellFiles,
  lib,
  libGL,
  libdisplay-info_0_3,
  libgbm,
  libinput,
  libxkbcommon,
  pango,
  pipewire,
  pixman,
  pkg-config,
  rustPlatform,
  seatd,
  stdenv,
  systemd,
  wayland,

  self,
}:
{
  pname,
  overrideCraneArgs ? craneArgs: craneArgs,
  overrideCraneBuildArgs ? craneBuildArgs: craneBuildArgs,
  buildFeatures ? "",
  ...
}@args:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../../Cargo.toml);
  craneArgs =
    let
      craneArgsPre = overrideCraneArgs {
        inherit (args) src pname;

        nativeBuildInputs = [
          autoPatchelfHook
          installShellFiles
          pkg-config
          rustPlatform.bindgenHook
        ]
        ++ (args.nativeBuildInputs or [ ]);

        runtimeDependencies = [
          libGL
          wayland
        ]
        ++ (args.runtimeDependencies or [ ]);

        buildInputs = [
          cairo
          dbus
          libGL
          libdisplay-info_0_3
          libinput
          seatd
          libxkbcommon
          libgbm
          pango
          wayland
        ]
        ++ (args.buildInputs or [ ]);

        cargoExtraArgs =
          "--locked --package ${pname} --no-default-features"
          + (lib.optionalString (buildFeatures != "") " --features ${buildFeatures}");

        # These tests require the ability to access a "valid EGL Display", but that won't work
        # inside the Nix sandbox
        cargoTestExtraArgs = "-- --skip=::egl";

        strictDeps = true;

        env = {
          RUSTFLAGS = "-Dwarnings";
        };
      };
    in
    craneArgsPre
    // {
      # ever since this commit:
      # https://github.com/YaLTeR/niri/commit/771ea1e81557ffe7af9cbdbec161601575b64d81
      # niri now runs an actual instance of the real compositor (with a mock backend) during tests
      # and thus creates a real socket file in the runtime dir.
      # this is fine for our build, we just need to make sure it has a directory to write to.
      preCheck =
        craneArgsPre.preCheck or ''
          export XDG_RUNTIME_DIR="$(mktemp -d)"
          export LD_LIBRARY_PATH=${
            builtins.concatStringsSep ":" (
              map (e: "${e.lib or e.out}/lib") (
                craneArgs.buildInputs
                ++ [
                  glib
                  pixman
                ]
              )
            )
          }
        '';
    };

  cargoArtifacts = craneLib.buildDepsOnly craneArgs;

  craneBuildArgs = overrideCraneBuildArgs (
    craneArgs
    // {
      inherit cargoArtifacts;
    }
  );
in
craneLib.buildPackage (
  craneBuildArgs
  // {
    version = cargoToml.workspace.package.version;

    postFixup = ''
      autoPatchelf $out/bin
    '';

    passthru = (args.passthru or { }) // {
      tests = (args.passthru.tests or { }) // {
        cargo-test = craneLib.cargoTest craneBuildArgs;
        cargo-clippy = craneLib.cargoClippy craneBuildArgs;
        cargo-audit = craneLib.cargoAudit (
          craneBuildArgs
          // {
            inherit advisory-db;
          }
        );
      };
    };

    env =
      craneArgs.env
      // (lib.optionalAttrs (self ? rev) {
        NIRI_BUILD_COMMIT = self.rev;
      })
      // (args.env or { });
  }
  // (builtins.removeAttrs args [
    "buildFeatures"
    "buildInputs"
    "nativeBuildInputs"
    "overrideCraneArgs"
    "overrideCraneBuildArgs"
    "passthru"
    "runtimeDependencies"
  ])
)
