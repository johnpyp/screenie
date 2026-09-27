# screenie, built from this repository. The flake passes `commit` and `commitDate`, which
# the build would otherwise read from git (see crates/screenie/build.rs).
{
  lib,
  rustPlatform,
  pkg-config,
  gst_all_1,
  pipewire,
  libxkbcommon,
  libxcb,
  libgbm,
  fontconfig,
  vulkan-loader,
  wayland,
  libGL,
  commit ? "unknown commit",
  commitDate ? "",
}:

let
  workspace = (lib.importTOML ../Cargo.toml).workspace.package;
  # What recording finds at runtime: encoders (x264, VA-API, NVENC), the MP4 muxer, GL
  # conversion, and PulseAudio/PipeWire capture.
  gstPlugins = with gst_all_1; [
    gstreamer
    gst-plugins-base
    gst-plugins-good
    gst-plugins-bad
    gst-plugins-ugly
    gst-libav
  ];
in
rustPlatform.buildRustPackage {
  pname = "screenie";
  inherit (workspace) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../patches
      ../assets
      ../README.md
      # Workspace members, which cargo reads even to build the app alone.
      ../tools/wlinput
      ../tools/wllock
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  buildType = "dist";
  cargoBuildFlags = [ "--package=screenie" ];
  # CI runs the tests; here they'd be rebuilt with the dist profile's LTO.
  doCheck = false;

  env = {
    SCREENIE_COMMIT = commit;
    SCREENIE_COMMIT_DATE = commitDate;
    # Scanned by the binary itself (screenie-record's `init`), rather than set by a
    # wrapper script: KWin grants screen capture to the binary a desktop entry runs, and
    # the entry has to run the binary itself.
    SCREENIE_GST_PLUGIN_PATH = lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gstPlugins;
  };

  nativeBuildInputs = [
    pkg-config
    # libclang, for PipeWire's bindings.
    rustPlatform.bindgenHook
  ];
  buildInputs = [
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    pipewire
    libxkbcommon
    libxcb
    libgbm
    fontconfig
  ];

  postInstall = ''
    $out/bin/screenie man $out/share/man
    $out/bin/screenie entry $out/share
  '';

  # The GPU and Wayland libraries are loaded at runtime rather than linked.
  postFixup = ''
    patchelf --add-rpath ${
      lib.makeLibraryPath [
        vulkan-loader
        wayland
        libGL
      ]
    } $out/bin/screenie
  '';

  meta = {
    description = "Screenshots and screen recordings for Wayland";
    homepage = workspace.repository;
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "screenie";
  };
}
