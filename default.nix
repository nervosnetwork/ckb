{ pkgs ? import (builtins.fetchTarball { # 2020-02-13 (nixos-19.09)
    url = "https://github.com/NixOS/nixpkgs/archive/e02fb6eaf70d4f6db37ce053edf79b731f13c838.tar.gz";
    sha256 = "1dbjbak57vl7kcgpm1y1nm4s74gjfzpfgk33xskdxj9hjphi6mws";
  }) {}
, rustChannelOf ? (import ("${builtins.fetchTarball { # 2020-02-07
    url = "https://github.com/mozilla/nixpkgs-mozilla/archive/969a1e467abbb91affbf0c24ddf773d2bdf70ccd.tar.gz";
    sha256 = "13j54pzd9sfyimcmzl0hahzhvr930kiqj839nyk7yxp3nr0zy2xx";
  }}/rust-overlay.nix") pkgs pkgs).rustChannelOf

# Rust manifest hash must be updated when rust-toolchain file changes.
, rustPackages ? rustChannelOf { # channel-rust-1.41.0.toml
    dist_root = "https://static.rust-lang.org/dist";
    rustToolchain = ./rust-toolchain;
    sha256 = "07mp7n4n3cmm37mv152frv7p9q58ahjw5k8gcq48vfczrgm5qgiy";
  } }:
let
  rustPlatform = pkgs.makeRustPlatform {
    inherit (rustPackages) cargo;
    rustc = rustPackages.rust;
  };
in rustPlatform.buildRustPackage {
  name = "ckb";
  src = ./.;
  nativeBuildInputs = [ pkgs.openssl pkgs.pkgconfig ];
  buildInputs = [ rustPackages.rust-std ];
  verifyCargoDeps = true;

  # Cargo hash must be updated when Cargo.lock file changes.
  cargoSha256 = "16hpn3bam4lqm5yfijld2dp8lrgp7azw95wizwcrcsdd369m6i5f";
}
