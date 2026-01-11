{
  overlay,
}:
{
  pkgs,
  lib,
  ...
}:
{
  nixpkgs.overlays = [
    overlay
  ];

  programs.niri = {
    package = pkgs.niriPackages.niri;
    useNautilus = false;
  };

  xdg.portal = {
    extraPortals = [
      pkgs.kdePackages.xdg-desktop-portal-kde
    ];
    config.niri = {
      "org.freedesktop.impl.portal.FileChooser" = lib.mkForce "kde";
    };
  };
}
