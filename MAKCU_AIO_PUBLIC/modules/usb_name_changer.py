import ctypes
import sys
import serial.tools.list_ports
import os
import signal
import subprocess
from colorama import init, Fore
import threading
import time

IS_WINDOWS = sys.platform.startswith("win")

# Windows-only APIs. These modules do not exist on macOS/Linux, so import them
# lazily and only when running on Windows. The registry/WMI/driver features that
# depend on them are gated on IS_WINDOWS throughout this module.
if IS_WINDOWS:
    import winreg
    import win32com.client
    import pythoncom
    from modules.utils import get_driver_path

init()

VID = 0x1A86
PID = 0x55D3
DEFAULT_NAME = "USB-Enhanced-SERIAL CH343"
TARGET_DESC = "USB-SERIAL CH340"
MAX_NAME_LENGTH = 40

class USBNameChanger:
    def __init__(self, logger, is_admin_func):
        self.logger = logger
        self.is_admin = is_admin_func
        self.vid = VID
        self.pid = PID
        self.target_desc = TARGET_DESC
        self.default_name = DEFAULT_NAME
        self.max_name_length = MAX_NAME_LENGTH
        self.driver_checked = False

    def is_device_connected(self):
        ports = serial.tools.list_ports.comports()
        for port in ports:
            if port.vid == self.vid and port.pid == self.pid:
                return True
        return False

    def get_device_info(self):
        ports = serial.tools.list_ports.comports()
        for port in ports:
            if port.vid == self.vid and port.pid == self.pid:
                return port.description or "Unknown", port.device
        return None, None

    def update_registry_name(self, new_name, com_port=None):
        if not IS_WINDOWS:
            self.logger.terminal_print(
                "USB FriendlyName editing is only supported on Windows."
            )
            return False
        key_path = f"SYSTEM\\CurrentControlSet\\Enum\\USB\\VID_{self.vid:04X}&PID_{self.pid:04X}"
        friendly_name = new_name[:self.max_name_length]

        try:
            key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, key_path, 0, winreg.KEY_ALL_ACCESS)
            for i in range(winreg.QueryInfoKey(key)[0]):
                subkey_name = winreg.EnumKey(key, i)
                subkey_path = f"{key_path}\\{subkey_name}"
                subkey = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, subkey_path, 0, winreg.KEY_ALL_ACCESS)
                winreg.SetValueEx(subkey, "FriendlyName", 0, winreg.REG_SZ, friendly_name)
                winreg.CloseKey(subkey)
            winreg.CloseKey(key)
            return True
        except Exception as e:
            self.logger.terminal_print(f"Registry update failed: {e}. Try running as Administrator.")
            return False

    def set_custom_name(self, custom_name):
        if not self.is_device_connected():
            self.logger.terminal_print("Device not found. Please insert the device.")
            return False

        device_name, com_port = self.get_device_info()
        if com_port:
            friendly_name = f"{custom_name} ({com_port})"
        else:
            friendly_name = custom_name

        if self.update_registry_name(friendly_name, com_port):
            self.logger.terminal_print(f"Device name set to {friendly_name}")
            return True

        return False

    def restore_original_name(self):
        if not self.is_device_connected():
            self.logger.terminal_print("Device not found. Please insert the device.")
            return False

        #self.logger.terminal_print("Reinstalling device to restore original CH343 name...")
        threading.Thread(target=self._reinstall_device, daemon=True).start()
        return True

    def _reinstall_device(self):
        if not IS_WINDOWS:
            self.logger.terminal_print(
                "Device reinstall is only supported on Windows."
            )
            return
        try:
            #self.logger.terminal_print("Reinstalling device to restore original CH343 name...")

            pythoncom.CoInitialize()
            wmi = win32com.client.GetObject("winmgmts:")

            query = f"SELECT * FROM Win32_PnPEntity WHERE DeviceID LIKE '%VID_{self.vid:04X}&PID_{self.pid:04X}%'"
            devices = wmi.ExecQuery(query)

            if not devices:
                self.logger.terminal_print("No matching devices found in WMI.")
                return

            for device in devices:
                #self.logger.terminal_print(f"Found device: {device.Name}")
                device_id = device.DeviceID

                try:
                    quoted_id = f'"{device_id}"'
                    #self.logger.terminal_print(f"Removing device: {device_id}")

                    subprocess.run(
                        f'pnputil /remove-device {quoted_id}',
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        shell=True
                    )

                    time.sleep(3)
                    self.logger.terminal_print("Please physically replug the USB device to complete re-enumeration.")
                    return

                except Exception as inner:
                    self.logger.terminal_print(f"Error uninstalling device: {inner}")

        except Exception as e:
            self.logger.terminal_print(f"Device reinstall error (WMI): {e}")


    def list_usb_devices(self):
        ports = serial.tools.list_ports.comports()
        dev_list = []
        for port in ports:
            vid = f"{port.vid:04X}" if port.vid else "N/A"
            pid = f"{port.pid:04X}" if port.pid else "N/A"
            name = port.description or "Unknown"
            dev_list.append(f"Port: {port.device}, VID: {vid}, PID: {pid}, Name: {name}")
        return dev_list

    def is_ch343_driver_installed(self):
        if not IS_WINDOWS:
            return True
        try:
            result = subprocess.run(
                ["pnputil", "/enum-drivers"],
                capture_output=True,
                text=True,
                shell=True
            )
            output = result.stdout.upper()
            return "CH343SER.INF" in output or "CH343" in output
        except Exception:
            return False

    def ensure_driver_installed(self):
        if not IS_WINDOWS:
            # No installable serial driver step on macOS/Linux; modern systems
            # ship CH34x/FTDI support, so treat the driver as already present.
            self.driver_checked = True
            return True
        if self.driver_checked:
            return True
        self.driver_checked = True

        if self.is_ch343_driver_installed():
            return True

        if not self.is_admin():
            self.logger.terminal_print("Admin rights required to install CH343 driver.")
            return False

        try:
            inf_path = get_driver_path("CH343SER.INF")
            self.logger.terminal_print("Installing CH343 driver...")

            result = subprocess.run(
                ["pnputil", "/add-driver", inf_path, "/install"],
                capture_output=True,
                text=True,
                shell=True
            )

            if result.returncode in (0, 3010):
                if result.returncode == 3010:
                    self.logger.terminal_print("CH343 driver installed. A system reboot may be required.")
                return True
            else:
                self.logger.terminal_print("Failed to install CH343 driver.")
                return False

        except Exception as e:
            self.logger.terminal_print(f"Driver installation error: {e}")
            return False


class USBNameChangerFTDI(USBNameChanger):
    def __init__(self, logger, is_admin_func):
        super().__init__(logger, is_admin_func)
        self.vid = 0x0403
        self.pid = 0x6001
        self.default_name = "USB Serial Port"
        self.target_desc = "USB-SERIAL CH340"
