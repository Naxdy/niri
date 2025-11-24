{ pkgs, ... }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
{
  programs.nixfmt.enable = true;

  programs.taplo.enable = true;

  programs.rustfmt = {
    enable = true;
    package = pkgs.niriPackages.niri.passthru.rustToolchain;
    edition = cargoToml.workspace.package.edition;
  };

  programs.typos = {
    enable = true;
    configFile = "${./typos.toml}";
    includes = [
      "*.md"
      "*.nix"
      "*.rs"
      "*.vert"
      "*.frag"
    ];
  };

  programs.prettier = {
    enable = true;
    settings = {
      singleQuote = true;
      trailingComma = "all";
      semi = true;
      printWidth = 120;
      proseWrap = "always";
    };
    includes = [
      "*README.md"
    ];
  };
}
