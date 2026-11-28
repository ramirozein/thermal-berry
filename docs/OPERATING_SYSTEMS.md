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

## Control de ventilación en Windows

El escritorio consulta datos térmicos de GPU expuestos por D3DKMT y obtiene zonas ACPI y tacómetros a través de WMI cuando el controlador los publica. La clase estándar `Win32_Fan` declara `SetSpeed` sin implementación. La interfaz térmica del kernel entrega solicitudes de refrigeración al controlador del dispositivo y coordina las respuestas con el sistema operativo. [Win32_Fan](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-fan) · [Interfaz térmica para controladores](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/thermal-management-implementation)

El estudio M0 deja la actuación en Windows pendiente de una interfaz documentada para el modelo concreto, que permita confirmar el boost aplicado y devolver el control al firmware. Hace falta validar en hardware los permisos, la recuperación al detener el proceso y la protección contra sobretemperatura. El alcance actual de Windows y macOS cubre el ajuste offline desde JSONL y las lecturas de escritorio disponibles en cada plataforma. El control físico de calibración se limita a Linux.
