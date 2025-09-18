# warning: this flake is probably terrible, whatever
{
  description = "clippyboard: a clipboard manager";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { nixpkgs, ... }:

    let
      lib = nixpkgs.lib;
      clippyboard-package = ./default.nix;
      systems = lib.intersectLists lib.systems.flakeExposed lib.platforms.linux;
      forAllSystems = lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system: { default = nixpkgs.${system}.callPackage clippyboard-package { }; });
      nixosModules.default = { lib, config, pkgs, ... }:
        let
          cfg = config.services.clippyboard;
          clippyboard = pkgs.callPackage clippyboard-package { };
        in
        {
          options.services.clippyboard = {
            enable = lib.mkEnableOption "Enable the clippyboard daemon and clippyboard program";
          };

          config = lib.mkIf cfg.enable {
            nixpkgs.overlays = [
              (final: prev: {
                clipboard = clippyboard;
              })
            ];
            systemd.user.services.clippyboard = {
              description = "a clipboard manager";
              wantedBy = [ "graphical-session.target" ];
              after = [ "graphical-session.target" ];
              serviceConfig = {
                ExecStart = lib.getExe' clippyboard "clippyboard-daemon";
              };
            };
            environment.systemPackages = [ clippyboard ];
          };
        };
    };
}
