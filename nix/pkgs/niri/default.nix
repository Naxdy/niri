{
  pkgs,

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
  withDbus ? true,
  withDinit ? false,
  withScreencastSupport ? true,
  withSystemd ? true,
}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../../../Cargo.toml);

  mkToolchain =
    p:
    let
      targetMap = {
        aarch64-linux = "aarch64-unknown-linux-gnu";
        x86_64-linux = "x86_64-unknown-linux-gnu";
      };

      target-toolchain = p.niriPackages.fenix.targets.${targetMap.${stdenv.hostPlatform.system}}.stable;

      toolchain = p.niriPackages.fenix.stable;
    in
    p.niriPackages.fenix.combine [
      toolchain.rustc
      toolchain.cargo
      toolchain.rust-src
      toolchain.rustfmt
      toolchain.clippy
      target-toolchain.rust-std
    ];

  craneLib' = craneLib.overrideToolchain mkToolchain;

  craneArgs =
    let
      buildFeatures = builtins.concatStringsSep "," (
        lib.optional withDbus "dbus"
        ++ lib.optional withDinit "dinit"
        ++ lib.optional withScreencastSupport "xdp-gnome-screencast"
        ++ lib.optional withSystemd "systemd"
      );
    in
    {
      nativeBuildInputs = [
        autoPatchelfHook
        installShellFiles
        pkg-config
        rustPlatform.bindgenHook
      ];

      runtimeDependencies = [
        libGL
        wayland
      ];

      src =
        let
          niriFilter = path: type: builtins.match ".*niri-source/(resources|src).*" path != null;
          srcFilter = path: type: (niriFilter path type) || (craneLib'.filterCargoSources path type);
        in
        lib.cleanSourceWith {
          src = builtins.path {
            path = ../../../.;
            name = "niri-source";
          };
          filter = srcFilter;
          name = "source";
        };

      # ever since this commit:
      # https://github.com/YaLTeR/niri/commit/771ea1e81557ffe7af9cbdbec161601575b64d81
      # niri now runs an actual instance of the real compositor (with a mock backend) during tests
      # and thus creates a real socket file in the runtime dir.
      # this is fine for our build, we just need to make sure it has a directory to write to.
      preCheck = ''
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
      ++ lib.optional (withDbus || withScreencastSupport || withSystemd) dbus
      ++ lib.optional withScreencastSupport pipewire
      # Also includes libudev
      ++ lib.optional withSystemd systemd;

      cargoExtraArgs =
        "--locked --no-default-features"
        + (lib.optionalString (buildFeatures != "") " --features ${buildFeatures}");

      # These tests require the ability to access a "valid EGL Display", but that won't work
      # inside the Nix sandbox
      cargoTestExtraArgs = "-- --skip=::egl";

      strictDeps = true;

      env = {
        RUSTFLAGS = "-Dwarnings";
      };
    };

  craneBuildArgs = craneArgs // {
    inherit cargoArtifacts;
  };

  cargoArtifacts = craneLib'.buildDepsOnly craneArgs;
in
craneLib'.buildPackage (
  craneBuildArgs
  // {
    pname = cargoToml.package.name;
    version = cargoToml.workspace.package.version;

    postPatch = ''
      patchShebangs resources/niri-session
      substituteInPlace resources/niri.service \
        --replace-fail '/usr/bin' "$out/bin"
    '';

    postInstall = ''
      install -Dm644 resources/niri.desktop -t $out/share/wayland-sessions
      install -Dm644 resources/niri-portals.conf -t $out/share/xdg-desktop-portal
    ''
    + lib.optionalString withSystemd ''
      install -Dm755 resources/niri-session $out/bin/niri-session
      install -Dm644 resources/niri{.service,-shutdown.target} -t $out/share/systemd/user
    '';

    postFixup = ''
      autoPatchelf $out/bin

      installShellCompletion --cmd niri \
        --bash <($out/bin/niri completions bash) \
        --fish <($out/bin/niri completions fish) \
        --nushell <($out/bin/niri completions nushell) \
        --zsh <($out/bin/niri completions zsh)
    '';

    passthru = {
      rustToolchain = mkToolchain pkgs;
      providedSessions = [ "niri" ];
      tests = {
        cargo-test = craneLib'.cargoTest craneBuildArgs;

        cargo-clippy = craneLib'.cargoClippy craneBuildArgs;
      };
    };

    meta = {
      description = "Scrollable-tiling Wayland compositor";
      homepage = "https://github.com/YaLTeR/niri";
      license = lib.licenses.gpl3Only;
      mainProgram = "niri";
      platforms = lib.platforms.linux;
      maintainers = [
        lib.maintainers.naxdy
      ];
    };
  }
)
