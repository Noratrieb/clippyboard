{ pkgs ? import <nixpkgs> { } }: pkgs.rustPlatform.buildRustPackage {
  name = "clippyboard";

  src = pkgs.lib.cleanSource ./.;

  buildInputs = with pkgs; [
    libGL
    libxkbcommon
    wayland
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXrandr
  ];

  cargoLock.lockFile = ./Cargo.lock;
}
