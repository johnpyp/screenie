{
  description = "Screenshots and screen recordings for Wayland";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # What `screenie --version` shows, like a build from git: `46bce20 2026-09-25 23:20`
      # (in UTC), or `46bce20-dirty …` from a working tree with changes.
      date = self.lastModifiedDate or "19700101000000";
      at = start: len: builtins.substring start len date;
      rev = self.shortRev or self.dirtyShortRev or "unknown";
      day = "${at 0 4}-${at 4 2}-${at 6 2}";
      stamp = {
        commit = "${rev} ${day} ${at 8 2}:${at 10 2}";
        commitDate = day;
      };
      screenie = pkgs: pkgs.callPackage ./nix/package.nix stamp;
    in
    {
      packages = forAllSystems (pkgs: {
        default = screenie pkgs;
        screenie = screenie pkgs;
      });

      overlays.default = final: _prev: { screenie = screenie final; };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ (screenie pkgs) ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
          ];
          # What the package's wrapper sets, for `cargo run`.
          GST_PLUGIN_SYSTEM_PATH_1_0 =
            with pkgs.gst_all_1;
            pkgs.lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" [
              gstreamer
              gst-plugins-base
              gst-plugins-good
              gst-plugins-bad
              gst-plugins-ugly
              gst-libav
            ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              vulkan-loader
              wayland
              libGL
            ]
          );
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
