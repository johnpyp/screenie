# What every desktop image shares: screenie's runtime (GStreamer with PipeWire, Vulkan in
# software), a D-Bus session, PipeWire, portals, fonts, and a terminal to capture.
FROM registry.fedoraproject.org/fedora:44
RUN dnf -y --setopt=install_weak_deps=False install \
      dbus-daemon dbus-tools procps-ng psmisc findutils which python3 python3-gobject \
      python3-dbus glib2 xdg-utils wl-clipboard wayland-utils foot \
      pipewire pipewire-utils wireplumber pipewire-gstreamer \
      gstreamer1 gstreamer1-plugins-base gstreamer1-plugins-good \
      gstreamer1-plugins-bad-free gstreamer1-plugins-ugly-free \
      mesa-dri-drivers mesa-vulkan-drivers vulkan-loader mesa-libgbm \
      libxkbcommon libxkbcommon-x11 fontconfig google-noto-sans-fonts \
      xdg-desktop-portal ImageMagick gstreamer1-plugin-openh264 \
    && dnf clean all
