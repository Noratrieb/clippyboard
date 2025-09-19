{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell rec {
  buildInputs = with pkgs; [
    expat
    fontconfig
    freetype
    freetype.dev
    libGL
    pkg-config
    wayland
    libxkbcommon
  ];

  CLIPPYBOARD_SOCKET = "./clippyboard.socket";

  LD_LIBRARY_PATH =
    builtins.foldl' (a: b: "${a}:${b}/lib") "${pkgs.vulkan-loader}/lib" buildInputs;
}

