{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib) types mkOption;

  cfg = config.wayland.windowManager.niri;

  toNiriKDL =
    { }:
    let
      inherit (lib)
        concatStringsSep
        mapAttrsToList
        any
        ;
      inherit (builtins) typeOf replaceStrings elem;

      # ListOf String -> String
      indentStrings =
        let
          # Although the input of this function is a list of strings,
          # the strings themselves *will* contain newlines, so you need
          # to normalize the list by joining and resplitting them.
          unlines = lib.splitString "\n";
          lines = lib.concatStringsSep "\n";
          indentAll = lines: concatStringsSep "\n" (map (x: "	" + x) lines);
        in
        stringsWithNewlines: indentAll (unlines (lines stringsWithNewlines));

      # String -> String
      sanitizeString = replaceStrings [ "\n" ''"'' ] [ "\\n" ''\"'' ];

      # OneOf [Int Float String Bool Null] -> String
      literalValueToString =
        element:
        lib.throwIfNot
          (elem (typeOf element) [
            "int"
            "float"
            "string"
            "bool"
            "null"
          ])
          "Cannot convert value of type ${typeOf element} to KDL literal."
          (
            if typeOf element == "null" then
              "null"
            else if element == false then
              "false"
            else if element == true then
              "true"
            else if typeOf element == "string" then
              ''"${sanitizeString element}"''
            else
              toString element
          );

      # Attrset Conversion
      # String -> AttrsOf Anything -> String
      convertAttrsToKDL =
        name: attrs:
        let
          optArgs = map literalValueToString (attrs._args or [ ]);
          optProps = lib.mapAttrsToList (name: value: "${name}=${literalValueToString value}") (
            attrs._props or { }
          );

          orderedChildren = lib.pipe (attrs._children or [ ]) [
            (map (child: mapAttrsToList convertAttributeToKDL child))
            lib.flatten
          ];
          unorderedChildren = lib.pipe attrs [
            (lib.filterAttrs (
              name: _:
              !(elem name [
                "_args"
                "_props"
                "_children"
              ])
            ))
            (mapAttrsToList convertAttributeToKDL)
          ];
          children = orderedChildren ++ unorderedChildren;
          optChildren = lib.optional (children != [ ]) ''
            {
            ${indentStrings children}
            }'';

        in
        lib.concatStringsSep " " ([ name ] ++ optArgs ++ optProps ++ optChildren);

      # List Conversion
      # String -> ListOf (OneOf [Int Float String Bool Null])  -> String
      convertListOfFlatAttrsToKDL =
        name: list:
        let
          flatElements = map literalValueToString list;
        in
        "${name} ${concatStringsSep " " flatElements}";

      # String -> ListOf Anything -> String
      convertListOfNonFlatAttrsToKDL = name: list: ''
        ${lib.concatStringsSep "\n" (map (x: convertAttributeToKDL name x) list)}
      '';

      # String -> ListOf Anything  -> String
      convertListToKDL =
        name: list:
        let
          elementsAreFlat =
            !any (
              el:
              elem (typeOf el) [
                "list"
                "set"
              ]
            ) list;
        in
        if elementsAreFlat then
          convertListOfFlatAttrsToKDL name list
        else
          convertListOfNonFlatAttrsToKDL name list;

      # Combined Conversion
      # String -> Anything  -> String
      convertAttributeToKDL =
        name: value:
        let
          vType = typeOf value;
        in
        if
          elem vType [
            "int"
            "float"
            "bool"
            "null"
            "string"
          ]
        then
          "${name} ${literalValueToString value}"
        else if vType == "set" then
          convertAttrsToKDL name value
        else if vType == "list" then
          convertListToKDL name value
        else
          throw ''
            Cannot convert type `(${typeOf value})` to KDL:
              ${name} = ${toString value}
          '';
    in
    attrs: ''
      ${concatStringsSep "\n" (mapAttrsToList convertAttributeToKDL attrs)}
    '';

  mkKDL = toNiriKDL { };
in
{
  options.wayland.windowManager.niri = {
    enable = lib.mkEnableOption "Niri, a scrollable tiling Wayland compositor";

    package = lib.mkPackageOption pkgs "niri" {
      nullable = true;
      extraDescription = "Set this to null if you use the NixOS module to install niri.";
    };

    settings = mkOption {
      type = types.submodule {
        freeformType = types.attrsOf types.anything;
      };
      default = { };
      description = ''
        KDL configuration for Niri written in Nix. Uses home manager's KDL generator.
      '';
      example = lib.literalExpression ''
        {
          input = {
            keyboard = {
              layout = "us";
            };
          };
          tablet {
            map-to-output = "e-DP-1";
          };
          binds = {
            "Mod+TouchpadScrollDown" = {
              _props = {
                cooldown-ms = 500;
              };
              focus-workspace-down = [];
            };
            "Mod+T" = {
              spawn = "alacritty";
            };
            XF86AudioRaiseVolume = {
              spawn-sh = [
                "wpctl" "set-volume" "@DEFAULT_AUDIO_SINK@" "0.1+"
              ];
            };
          };
        }
      '';
    };

    extraConfig = mkOption {
      type = types.lines;
      default = "";
      description = ''
        Extra configuration lines to be added verbatim.
      '';
    };

    systemd = {
      variables = lib.mkOption {
        type = types.listOf types.str;
        default = [
          "DISPLAY"
          "HYPRLAND_INSTANCE_SIGNATURE"
          "WAYLAND_DISPLAY"
          "XDG_CURRENT_DESKTOP"
        ];
        example = [ "--all" ];
        description = ''
          Environment variables to be imported in the systemd & D-Bus user
          environment.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];

    xdg.configFile."niri/config.kdl" = {
      text = (mkKDL cfg.settings) + "\n" + cfg.extraConfig;
    };
  };
}
