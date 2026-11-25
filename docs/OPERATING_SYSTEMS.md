# Operating systems

Thermal Berry reads the sensors exposed by the installed kernel or Windows drivers. Available readings depend on the model, firmware and installed driver.

| Platform | Packages |
| --- | --- |
| Linux x86-64 | APT repository for Debian and Ubuntu, AppImage, DEB, and RPM |
| Windows | NSIS installer (`.exe`) |
| macOS | Universal disk image (`.dmg`) for Intel and Apple Silicon |

## Thermal compatibility

| Device family | Linux | Windows | Physical validation of new support |
| --- | --- | --- | --- |
| Lenovo consumer (Yoga, IdeaPad, Legion and related models) | Read-only temperatures and RPM with supported `hwmon` sources such as `yogafan` | Read-only GPU temperature and RPM, plus ACPI zones and WMI tachometers when available | Pending model, firmware and OS/driver version |
| Lenovo ThinkPad | Read-only temperatures and RPM with `thinkpad_acpi` | Read-only GPU temperature and RPM, plus ACPI zones and WMI tachometers when available | Pending model, firmware and OS/driver version |
| HP | Read-only temperatures and RPM with `hp-wmi` or `hp-wmi-sensors` | Read-only GPU temperature and RPM, plus ACPI zones and WMI tachometers when available | Pending model, firmware and OS/driver version |
| ASUS | Read-only temperatures and RPM with supported ASUS `hwmon` sources | Read-only GPU temperature and RPM, plus ACPI zones and WMI tachometers when available | Pending model, firmware and OS/driver version |
| Dell / Alienware | Temperatures, RPM and fan boost on devices with `alienware_wmi` | Read-only GPU temperature and RPM, plus ACPI zones and WMI tachometers when available | Existing Linux backend |