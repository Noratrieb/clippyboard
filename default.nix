{ pkgs ? import <nixpkgs> { } }: pkgs.rustPlatform.buildRustPackage rec {
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


  nativeBuildInputs = with pkgs; [
    pkg-config
    cmake
    makeWrapper
  ];

  postFixup = ''
    wrapProgram $out/bin/clippyboard-select \
      --suffix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath buildInputs}
    wrapProgram $out/bin/clippyboard-daemon \
      --suffix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath buildInputs}
  '';

  cargoLock.lockFile = ./Cargo.lock;
}
