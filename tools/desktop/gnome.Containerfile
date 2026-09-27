ARG COMMON=localhost/screenie-desktop-common:44
FROM ${COMMON}
RUN dnf -y --setopt=install_weak_deps=False install \
      gnome-shell mutter gnome-settings-daemon gsettings-desktop-schemas \
      xdg-desktop-portal-gnome xdg-desktop-portal-gtk gnome-text-editor dconf \
    && dnf clean all
