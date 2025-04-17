{ lib, pkgs }:
pkgs.callPackage (
  { rustPlatform }:
  rustPlatform.buildRustPackage {
    name = "dbusgraft";
    src = ./.;
    cargoLock = {
      lockFile = ./Cargo.lock;
      outputHashes = {
        "zbus-5.5.0" = "sha256-4iMmDzygZvSKxps1/8RNJRfWQktskVw4FYEPAt7D5Y8=";
      };
    };
    nativeBuildInputs = [
      # pkgs.pkg-config
      # pkgs.llvmPackages.libclang
    ];
    buildInputs = [
      # pkgs.dbus
    ];
  }
) { }
