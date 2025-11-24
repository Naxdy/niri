# niri

This repo houses a fork of [niri](https://github/com/YaLTeR/niri), a scrollable tiling Wayland compositor.

This fork changes the behavior of upstream niri in a few ways that do not necessarily align with upstream's vision for
the project (hence the fork).

If you are on NixOS, an easy way to try out this fork is using the provided flake (more at the bottom of this readme).

This readme outlines the differences between this fork and upstream niri. For more info on upstream's version, check out
the [original readme](./README_orig.md).

## New Features

### Blur

Windows (both floating and tiling), as well as layer surfaces can have blur enabled on them. Blur needs to be enabled
for each window / layer surface explicitly.

Tiled windows will draw "optimized" blur that is rendered using only `bottom` and `background` layer surfaces. Floating
windows, as well as `top` and `overlay` layer surfaces will draw "true" blur, that is rendered in an extra pass using
all visible screen contents.

To set global defaults for blur:

```kdl
layout {
  blur {
    noise 0
    passes 4
    radius 12
  }
}
```

To enable blur for a specific window / layer surface:

```kdl
window-rule {
  match app-id="kitty"
  blur {
    // will enable blur with defaults
    on
  }
}
window-rule {
  match app-id="org.telegram.desktop"
  blur {
    // will enable blur with custom `noise` setting
    // note that this only affects the window _while it is floating_, as
    // tiled windows all share the same optimized blur texture
    on
    noise 4
  }
}
layer-rule {
  match namespace="swaync-notification-window"

  // blur will adjust to `geometry-corner-radius`
  geometry-corner-radius 4

  blur {
    on
  }
}
```

#### Caveats

- Floating windows currently blur incorrectly in the overview (the blur texture is zoomed-out twice).
- As of right now, it is only possible to produce blur in rectangular shapes depending on the target surface size, with
  or without rounded corners (though all corners have to be rounded equally).

  In the future, the plan is to introduce a setting that allows masking blur pixel-by-pixel, similar to Hyprland's
  `ignorealpha` setting, though this will require careful GPU profiling.

- Blur is currently only possible to be enabled through the config. Implementing both
  [KDE blur](https://wayland.app/protocols/kde-blur) and
  [background effect](https://wayland.app/protocols/ext-background-effect-v1) is planned though.

### Window Groups (Tabbed Tiles)

Tiles can be turned into grouped tiles via the `toggle-group` action. Other windows can then be moved into our out of a
group via the `move-window-into-or-out-of-group` action, that accepts a directional parameter. Tabs can be cycled via
the `focus-next-window` and `focus-previous-window` actions. Example config:

```kdl
binds {
  Mod+G {
    toggle-group
  }
  Mod+Tab {
    focus-next-window
  }
  Mod+Shift+H {
    move-window-into-or-out-of-group "left"
  }
  Mod+Shift+L {
    move-window-into-or-out-of-group "right"
  }
  Mod+Shift+K {
    move-window-into-or-out-of-group "up"
  }
  Mod+Shift+J {
    move-window-into-or-out-of-group "down"
  }
}
```

When using `move-window-into-or-out-of-group` on a non-grouped tile, but there is no suitable grouped tile in the
direction you're attempting to move to, the behavior will instead be similar to `consume-or-expel-window`, `-left` or
`-right` respectively, or `move-window`, `-up` or `-down` respectively.

#### Caveats

- When maximizing or fullscreening a grouped tile, the tab indicator will disappear. However, you can still cycle tabs
  using `focus-next-window` and `focus-previous-window`. The newly focused windows will assume the requested maximized /
  fullscreen size upon activating.
- When using `toggle-group` on a single window, the resize animation is a little bit jerky, due to being anchored at the
  top as opposed to anchored at the bottom. Personally, I'm fine with it, but if you happen to be bothered by it and fix
  it on your own branch, feel free to send a patch.

## Removed Features

### Tabbed Columns

Since windows can be grouped on a per-tile basis, column-level tabbing is obsolete. All associated code and config
options have been removed to improve maintainability. If you have any tabbed-column related options in your niri config,
this fork will fail to parse it.

## Plans

As of right now, I am trying to keep this fork "as close to upstream as is reasonable", to allow for frequent rebasing
without too many conflicts to solve.

However, in the future, I plan to make several more moderate-to-big changes to this fork, which will cause it to further
diverge from upstream. Once the point is reached where rebasing is no longer feasible, and I have not yet moved on to
TheNextShinyThing™, a rebrand is likely to happen, also to avoid confusion with the upstream project.

### KDE Screencasting

Above all, and next on my agenda, I'd like to implement the
[KDE screencast](https://wayland.app/protocols/kde-zkde-screencast-unstable-v1) wayland protocol, since
`xdg-desktop-portal-kde` provides a UI for region screencasting since
[my PR was merged upstream](https://invent.kde.org/plasma/xdg-desktop-portal-kde/-/merge_requests/161).

Although niri does have dynamic screencasting already, which is an amazing feature (that I fully intend to keep),
regional screencasting provides some additional functionality that I miss, such as streaming two or more windows side by
side, without having to share the entire screen.

I have yet to ascertain the feasibility of this venture however, since `xdg-desktop-portal-kde` interacts with `kwin` in
more ways behind the scenes, e.g. to show live window share previous, and I haven't yet determined which of these "extra
features" are requirements for the compositor to support, and which are optional to implement.

### More KDE Protocols

In general, niri leans in pretty heavily into Gnome for its portal functionality. Given that I'm more familiar with KDE,
and also work on KDE software myself every now and again, I plan to rewrite this fork to use more of KDE's stuff
instead.

Some examples include the ability to view (and perhaps change?) monitor settings from KDE's system settings, and
supporting [KDE's blur protocol](https://wayland.app/protocols/kde-blur), among others.

### Refactors & Macros

There are a couple areas of the code that would benefit from refactors and / or custom macros to improve both
maintainability and readability. One such example is
[this implementation of `From<niri_ipc::Action>`](https://github.com/YaLTeR/niri/blob/79e41d7d88de44356b48400515076bf5593544e8/niri-config/src/binds.rs#L390-L698).

This also includes raising the MSRV, and upgrading this project's edition from 2021 to 2024, since this will provide
many code quality features, with one of my most-wanted being
[let chains](https://doc.rust-lang.org/nightly/edition-guide/rust-2024/let-chains.html).

## Flake

This project provides a flake, intended to be used with NixOS and / or
[home-manager](https://github.com/nix-community/home-manager).

To use it, simply import the module it provides into your config:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    niri.url = "github:Naxdy/niri";

    # optional, if you use home-manager
    home-manager.url = "github:nix-community/home-manager";
  };

  outputs = { self, nixpkgs, niri }: {
    nixosConfigurations.my-system = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        # optional, if you use home-manager
        home-manager.nixosModules.default

        niri.nixosModules.default
        ({ config, lib, pkgs, ... }: {
          # takes care of setting up portals & other system services
          programs.niri.enable = true;

          # I highly recommend using UWSM, as it makes session management extremely convenient
          programs.uwsm = {
            enable = true;
            waylandCompositors.niri = {
              prettyName = "niri";
              comment = "niri compositor (fork) managed by UWSM";
              binPath = "/run/current-system/sw/bin/niri";
            };
          };

          environment.systemPackages = [
            pkgs.xwayland-satellite
          ];

          # optional, if using home-manager
          home-manager = {
            # recommended, as the niri module from this fork overrides the upstream niri package
            useGlobalPkgs = true;
            users.my-username = {
              imports = [
                niri.homeManagerModules.default
              ];

              wayland.windowManager.niri = {
                enable = true;
                # fully declarative niri configuration; converted to kdl during rebuild
                #
                # - simple entries without arguments are declared as `name = [];`
                # - named arguments are declared using `_props`
                # - multiple entries with the same name and different contents are declared using `_children`
                #
                # one caveat in this config is that toplevel primitive type options cannot be declared multiple times,
                # e.g., you can only have one `spawn-at-startup` entry here, but at least that shouldn't matter
                # too much when using UWSM.
                settings = {
                  layout = {
                    preset-window-heights._children = [
                      { proportion = 0.33333; }
                      { proportion = 0.5; }
                      { proportion = 0.66667; }
                    ];
                  };

                  window-rule = [
                    # applies to all windows
                    {
                      geometry-corner-radius = 10;
                      clip-to-geometry = true;
                      draw-border-with-background = false;
                    }
                    # single match
                    {
                      match._props.app-id = "kitty";
                      opacity = 0.885;
                    }
                    # multiple matches
                    {
                      match = [
                        { _props.app-id = "kitty"; }
                        { _props.app-id = "org.telegram.desktop"; }
                      ];

                      blur = {
                        on = [ ];
                      };
                    }
                  ];

                  binds = {
                    XF86AudioPause = {
                      _props.allow-when-locked = true;
                      spawn = [
                        "playerctl"
                        "play-pause"
                      ];
                    };
                    XF86AudioPlay = {
                      _props.allow-when-locked = true;
                      spawn = [
                        "playerctl"
                        "play-pause"
                      ];
                    };
                    "Mod+Shift+P".maximize-window-to-edges = [ ];
                    "Mod+Shift+S".spawn = [
                      "flameshot"
                      "gui"
                    ];
                  };
                };
              };
            };
          };
        })
      ];
    };
  };
}
```
