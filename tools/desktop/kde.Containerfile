FROM localhost/screenie-desktop-common
RUN dnf -y --setopt=install_weak_deps=False install \
      kwin kwin-wayland xdg-desktop-portal-kde kglobalacceld spectacle kscreen \
      qt6-qttools kde-cli-tools kf6-kconfig plasma-workspace konsole \
    && dnf clean all
# kwin_wayland carries cap_sys_nice, which a rootless container can't grant.
RUN setcap -r /usr/bin/kwin_wayland
