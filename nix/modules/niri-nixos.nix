{
  overlay,
}:
{
  config,
  lib,
  pkgs,
  ...
}:
{
  nixpkgs.overlays = [
    overlay
  ];

  programs.niri.package = pkgs.niriPackages.niri;

  xdg.portal = lib.mkIf config.programs.niri.enable {
    enable = true;
    xdgOpenUsePortal = lib.mkDefault true;
    extraPortals = [
      pkgs.kdePackages.xdg-desktop-portal-kde
      pkgs.xdg-desktop-portal-gnome
      pkgs.xdg-desktop-portal-gtk
    ];
    config = {
      kde = {
        default = [
          "kde"
          "gtk"
        ];
        "org.freedesktop.impl.portal.Screenshot" = "kde";
        "org.freedesktop.impl.portal.ScreenCast" = "gnome";
      };
    };
  };
}
