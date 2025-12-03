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
  libdisplay-info,
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
  withDbus ? true,
  withDinit ? false,
  withSystemd ? true,
  withScreencastSupport ? true,
  ...
}@args:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../../Cargo.toml);
  craneArgs =
    let
      workspaceBuildFeatures = builtins.concatStringsSep "," (
        lib.optional withDbus "dbus"
        ++ lib.optional withDinit "dinit"
        ++ lib.optional withScreencastSupport "xdp-gnome-screencast"
        ++ lib.optional withSystemd "systemd"
      );

      craneArgsPre = overrideCraneArgs {
        src =
          let
            niriFilter = path: type: builtins.match ".*niri-source/(resources|src).*" path != null;
            niriVisualTestsFilter =
              path: type: builtins.match ".*niri-source/niri-visual-tests/(resources|src).*" path != null;
            srcFilter =
              path: type:
              (niriFilter path type)
              || (craneLib.filterCargoSources path type)
              || (niriVisualTestsFilter path type);
          in
          lib.cleanSourceWith {
            src = builtins.path {
              path = ../../.;
              name = "niri-source";
            };
            filter = srcFilter;
            name = "source";
          };

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
          libdisplay-info
          libinput
          seatd
          libxkbcommon
          libgbm
          pango
          wayland
        ]
        ++ (args.buildInputs or [ ])
        ++ lib.optional (withDbus || withScreencastSupport || withSystemd) dbus
        ++ lib.optional withScreencastSupport pipewire
        # Also includes libudev
        ++ lib.optional withSystemd systemd;

        cargoExtraArgs =
          "--locked --no-default-features"
          + (lib.optionalString (workspaceBuildFeatures != "") " --features ${workspaceBuildFeatures}");

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
      cargoExtraArgs =
        "--locked --package ${pname} --no-default-features"
        + (lib.optionalString (buildFeatures != "") " --features ${buildFeatures}");

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
    "overrideCraneArgs"
    "overrideCraneBuildArgs"
    "passthru"
    "withDbus"
    "withDinit"
    "withScreencastSupport"
    "withSystemd"
  ])
)
