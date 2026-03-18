{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          rust-overlay.overlays.default
          self.overlays.default
        ];
      };
    in
    {
      overlays.default =
        final: _prev:
        let
          craneLib = (crane.mkLib final).overrideToolchain (final.rust-bin.stable.latest.default);

          commonArgs = {
            src =
              let
                # cleanCargoSource strips non-Rust files; extend it to also keep
                # any `assets/` directories (e.g. train/assets/*.bdf).
                isAsset = path: _type: builtins.match ".*/assets/.*" path != null;
                isCargoOrAsset = path: type: (craneLib.filterCargoSources path type) || (isAsset path type);
              in
              final.lib.cleanSourceWith {
                src = craneLib.path ./.;
                filter = isCargoOrAsset;
              };
            # Crane can't infer a name from a workspace Cargo.toml (no [package]
            # section), so we provide one explicitly to silence the warning.
            pname = "oryx";
            version = "0.1.0";
            strictDeps = true;
            nativeBuildInputs = [ final.pkg-config ];
            buildInputs = [
              final.hidapi
              final.udev
              final.dbus
            ];
          };

          # Build workspace deps once; all per-crate derivations reuse this.
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          mkBin =
            pname:
            craneLib.buildPackage (
              commonArgs
              // {
                inherit cargoArtifacts pname;
                cargoExtraArgs = "-p ${pname}";
                doCheck = false;
              }
            );
        in
        {
          oryx-ctl = mkBin "oryx-ctl";
          oryx-look = mkBin "oryx-look";
          oryx-jobs = mkBin "oryx-jobs";
          oryx-train = mkBin "oryx-train";
        };

      packages.${system} = {
        inherit (pkgs)
          oryx-ctl
          oryx-look
          oryx-jobs
          oryx-train
          ;
        default = pkgs.symlinkJoin {
          name = "oryx";
          paths = [
            pkgs.oryx-ctl
            pkgs.oryx-look
            pkgs.oryx-jobs
            pkgs.oryx-train
          ];
        };
      };

      nixosModules.default = import ./nix/oryx-jobs.nix;

      devShells.${system}.default = pkgs.mkShell {
        buildInputs = [
          pkgs.rust-bin.stable.latest.default
          pkgs.pkg-config
          pkgs.hidapi
          pkgs.udev
          pkgs.dbus
        ];
      };
    };
}
