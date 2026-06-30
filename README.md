# Arch Install Manager

destbg's arch install manager (**daim**) — a Linux Mint inspired GTK4-based
install, update, and package manager for Arch Linux.

![Arch Install Manager](showcase.png)

## Binaries

- `daim-gui` — the graphical application (Install / Update / Manage tabs)
- `daim` — the command-line engine (a focused, MIT-licensed AUR helper)
- `daim-helper` — a polkit-activated root helper; the GUI elevates **once** per
  session and reuses it for every privileged action
- `daim-tray` / `daim-check` — the systemd-driven tray applet and update check

## Features

- **Install** packages from the official repositories, the AUR, and Flatpak
- **Update** available packages from pacman, the AUR, Flatpak, and AppImage
- **Manage** installed packages: remove, downgrade, clean orphans and the cache
- Built-in AUR support — no external helper required; PKGBUILDs are shown for
  review before building, and builds run as your user (never as root)
- Boots as a normal user and asks for administrator rights only once, via polkit
- Create a Timeshift or Snapper snapshot before updates
- System tray icon with update notifications, started by systemd
- Mark packages as favorites, blacklist packages, sort and group them
- Switch a package between repositories, for example aur to extra
- Post update checks for service restarts and pacnew files

## Installing

```bash
makepkg -si
```
Or using the AUR
```bash
paru arch-install-manager
```

There are three AUR packages:
- `arch-install-manager` builds from the latest tagged release
- `arch-install-manager-bin` installs the prebuilt binary from that release
- `arch-install-manager-git` builds from the latest commit on main

## License

This project is licensed under the MIT License.
