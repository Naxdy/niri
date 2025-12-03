{
  advisory-db,
  crane,
  fenix,
  generateSplicesForMkScope,
  makeScopeWithSplicing',
  self,
  stdenv,
}:
makeScopeWithSplicing' {
  otherSplices = generateSplicesForMkScope "niriPackages";
  extra =
    final:
    let
      mkToolchain =
        p:
        let
          fenix' = p.callPackage fenix { };

          targetMap = {
            aarch64-linux = "aarch64-unknown-linux-gnu";
            x86_64-linux = "x86_64-unknown-linux-gnu";
          };

          target-toolchain = fenix'.targets.${targetMap.${stdenv.hostPlatform.system}}.stable;

          toolchain = fenix'.stable;
        in
        fenix'.combine [
          toolchain.rustc
          toolchain.cargo
          toolchain.rust-src
          toolchain.rustfmt
          toolchain.clippy
          target-toolchain.rust-std
        ];
    in
    {
      inherit
        advisory-db
        self
        ;

      craneLib = (crane.mkLib final).overrideToolchain mkToolchain;
      rustToolchain = mkToolchain final;
      mkNiriDerivation = final.callPackage ./nix/util/niri-derivation.nix { };
    };
  f = final: {
    callPackage' = pkg: attrs: (final.callPackage pkg attrs) // { niriPackage = true; };

    niri = final.callPackage' ./nix/pkgs/niri { };
    niri-visual-tests = final.callPackage' ./nix/pkgs/niri-visual-tests { };
  };
}
