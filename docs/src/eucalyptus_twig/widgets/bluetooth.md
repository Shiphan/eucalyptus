# Bluetooth

This widget depends on BlueZ D-Bus service to get information.

## Configure Options

- `settings_command`: What command to execute when you click the widget, useful for launching an external settings app

```toml
[widget.bluetooth]
settings_command = ["blueman-manager"]
```

