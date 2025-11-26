{
  overlay,
}:
{
  pkgs,
  ...
}:
{
  nixpkgs.overlays = [
    overlay
  ];

  programs.niri.package = pkgs.niriPackages.niri;
}
