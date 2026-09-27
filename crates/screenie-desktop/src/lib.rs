//! Fitting screenie into the desktop it runs on: knowing which one that is ([`Desktop`]),
//! being an app it knows ([`entry`]), having its screenshot keys ([`shortcuts`]), and on
//! GNOME, its Shell extension ([`extension`], [`shell`]).

pub mod clipboard;
pub mod entry;
pub mod extension;
mod gsettings;
pub mod notify;
pub mod overview;
pub mod shell;
pub mod shortcuts;

/// The desktop screenie runs on, where it matters: GNOME and KDE Plasma have their own
/// ways (of capturing, of placing windows, of binding keys), and everything else is a
/// Wayland compositor with the usual protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    /// GNOME Shell (Mutter), or a distribution's flavour of it.
    Gnome,
    /// KDE Plasma (KWin).
    Kde,
    Other,
}

impl Desktop {
    /// From `XDG_CURRENT_DESKTOP`.
    pub fn current() -> Desktop {
        let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let kde_session = std::env::var("KDE_FULL_SESSION").is_ok_and(|v| v == "true");
        Desktop::from_names(&desktops, kde_session)
    }

    fn from_names(desktops: &str, kde_session: bool) -> Desktop {
        let names: Vec<String> = desktops
            .split(':')
            .map(|d| d.trim().to_ascii_lowercase())
            .collect();
        let has = |name: &str| names.iter().any(|n| n == name);
        // Desktops built on GNOME's libraries, but not its shell, name it after their own.
        const NOT_GNOME_SHELL: &[&str] = &["budgie", "pantheon", "unity", "x-cinnamon", "mate"];
        if has("kde") || kde_session {
            Desktop::Kde
        } else if has("gnome") && !NOT_GNOME_SHELL.contains(&names[0].as_str()) {
            Desktop::Gnome
        } else {
            Desktop::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Desktop::Gnome => "GNOME",
            Desktop::Kde => "KDE Plasma",
            Desktop::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_are_known_by_their_names() {
        assert_eq!(Desktop::from_names("GNOME", false), Desktop::Gnome);
        assert_eq!(Desktop::from_names("ubuntu:GNOME", false), Desktop::Gnome);
        assert_eq!(Desktop::from_names("Budgie:GNOME", false), Desktop::Other);
        assert_eq!(Desktop::from_names("KDE", false), Desktop::Kde);
        assert_eq!(Desktop::from_names("", true), Desktop::Kde);
        assert_eq!(Desktop::from_names("sway", false), Desktop::Other);
        assert_eq!(Desktop::from_names("Hyprland", false), Desktop::Other);
    }
}
