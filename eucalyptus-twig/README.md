# Eucalyptus Twig

A status bar for Wayland

> [!NOTE]
> There is no system tray nor power menu for now.

## Config

Eucalyptus Twig read a toml file at `$XDG_CONFIG_HOME/eucalyptus-twig/eucalyptus-twig.toml`

An example would be:
```toml
theme = "Kanagawa Wave"

left = [
  "Power",
  "Clock",
]
middle = [
  "Workspaces",
]
right = [
  "Volume",
  ["Network", "Bluetooth"],
  "SystemInformation",
  "PowerProfile",
]

[widget.clock]
format = "%-m/%-d %a %-I:%M %p"

[widget.power_profile]
cycle_direction = "Down"

[widget.system_information]
update = 1 # s
temperature_hardware_name = "k10temp"

[widget.bluetooth]
settings_command = ["blueman-manager"]

[widget.network]
settings_command = ["alacritty", "--command", "nmtui"]

[widget.volume]
settings_command = ["pwvucontrol"]

[widget.workspaces]
show_hidden_workspace = true
```

---

TODO:
- [ ] power menu
    - [ ] functionality
    - [ ] animation
- [ ] battery/power
    - upower (dbus): <https://upower.freedesktop.org/docs/>
    - [x] icon with real percentage
    - [ ] more info: time to empty, energy rate
- [x] clock
    - [x] analog clock icon
- [ ] wayland/xwayland (hyprland)
- [ ] systray
    - dbus: <https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/>
- [x] workspaces (hyprland)
    - hyprland ipc: <https://wiki.hypr.land/IPC/>
- [x] workspaces (wayland)
    - <https://wayland.app/protocols/ext-workspace-v1>
- [ ] audio/volume
    - pipewire: <https://gitlab.freedesktop.org/pipewire/pipewire-rs>
    - pipewire-native: <https://gitlab.freedesktop.org/pipewire/pipewire-native-rs>
    - [x] show info: TODO: kinda works but not perfect
    - [ ] setting panel
- [ ] internet/wifi
    - networkmanager (dbus): <https://networkmanager.dev/docs/api/latest/spec.html>
    - [x] show info
    - [ ] setting panel
- [ ] bluetooth
    - bluez: <https://github.com/bluez/bluer>
    - [x] show info
    - [ ] setting panel
- [x] system info
    - [x] cpu
    - [x] ram
    - [x] temperature
- [ ] power profile (power-profile-daemon)
    - [x] show info
    - dbus: <https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html>
    - [x] setting panel
    - [ ] maybe also support tlp (same dbus api as ppd): TODO: test tlp support
- [ ] notification
    - dbus: <https://specifications.freedesktop.org/notification/latest/>

