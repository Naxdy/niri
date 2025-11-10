{
  crane,
  fenix,
  generateSplicesForMkScope,
  makeScopeWithSplicing',
  self,
}:
makeScopeWithSplicing' {
  otherSplices = generateSplicesForMkScope "niriPackages";
  extra = final: {
    inherit self;

    craneLib = crane.mkLib final;
  };
  f = final: {
    fenix = final.callPackage fenix { };

    callPackage' = pkg: attrs: (final.callPackage pkg attrs) // { niriPackage = true; };

    niri = final.callPackage' ./nix/pkgs/niri { };
  };
}
