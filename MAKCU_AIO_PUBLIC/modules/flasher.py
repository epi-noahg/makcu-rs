import requests
import threading
import os
import subprocess
import time
import sys
from modules.utils import get_download_path, get_icon_path

IS_WINDOWS = sys.platform.startswith("win")

class Flasher:
    """
    Handles flashing firmware BIN files using pre-downloaded files when available.
    Falls back to online download if local files are missing.
    Firmware details are provided by ConfigManager and include ``name``,
    ``version``, ``changelog``, ``primary_url``, and ``fallback_url`` for
    each side.
    """
    def __init__(self, logger, serial_handler, config_manager):
        self.logger = logger
        self.serial_handler = serial_handler
        self.config_manager = config_manager
        if IS_WINDOWS:
            # Windows ships a bundled esptool.exe (also used by frozen builds).
            try:
                self.esptool_path = get_icon_path('esptool.exe')
                if not os.path.exists(self.esptool_path):
                    raise FileNotFoundError(f"esptool.exe not found at {self.esptool_path}")
            except FileNotFoundError as e:
                self.logger.terminal_print(f"Esptool not found: {e}")
                raise
            self.esptool_cmd = [self.esptool_path]
        else:
            # macOS/Linux run esptool as a Python module (pip-installed).
            self.esptool_path = None
            self.esptool_cmd = [sys.executable, '-m', 'esptool']
        self.is_flashing = False
        self.flashing_lock = threading.Lock()  # Ensure only one flashing process runs at a time

    def download_and_flash(self, firmware_key):
        """Flash firmware for the specified side or firmware key, downloading if needed."""
        def task():
            info = self.config_manager.get_firmware_info(firmware_key)
            if not info:
                self.logger.terminal_print(
                    f"No firmware information for side: {firmware_key}"
                )
                return
            bin_filename = info["filename"]
            bin_path = get_download_path(bin_filename)
            if self.config_manager.is_bin_downloaded(info["name"]):
                self.logger.terminal_print(f"Using pre-downloaded {bin_filename} at {bin_path}")
                self.flash_firmware(bin_path)
                return

            if not self.config_manager.is_online_status():
                self.logger.terminal_print(
                    f"Offline mode detected. Cannot download {bin_filename}. Local file missing."
                )
                return

            try:
                primary_url, fallback_url = self.config_manager.get_firmware_urls(
                    firmware_key
                )
                primary_server = "GitHub"
                fallback_server = "Gitee"
                last_successful_server = self.config_manager.get_config_value("last_successful_server")
                if last_successful_server == "gitee":
                    primary_url, fallback_url = fallback_url, primary_url
                    primary_server, fallback_server = fallback_server, primary_server
                    self.logger.terminal_print(f"Prioritizing {fallback_server} for {bin_filename}")

                response = requests.get(primary_url, stream=True, timeout=10)
                response.raise_for_status()
                with open(bin_path, 'wb') as f:
                    for chunk in response.iter_content(chunk_size=8192):
                        f.write(chunk)
                self.logger.terminal_print(
                    f"Downloaded {bin_filename} from {primary_server} to {bin_path}"
                )
                self.config_manager.set_config_value("last_successful_server", primary_server.lower())
                self.config_manager.set_config_value("is_online", True)
                self.flash_firmware(bin_path)

            except requests.RequestException as e:
                self.logger.terminal_print(
                    f"Failed to download {bin_filename} from {primary_server}: {e}"
                )
                try:
                    response = requests.get(fallback_url, stream=True, timeout=10)
                    response.raise_for_status()
                    with open(bin_path, 'wb') as f:
                        for chunk in response.iter_content(chunk_size=8192):
                            f.write(chunk)
                    self.logger.terminal_print(
                        f"Downloaded {bin_filename} from {fallback_server} to {bin_path}"
                    )
                    self.config_manager.set_config_value(
                        "last_successful_server", fallback_server.lower()
                    )
                    self.config_manager.set_config_value("is_online", True)
                    self.flash_firmware(bin_path)
                except requests.RequestException as e:
                    self.logger.terminal_print(
                        f"Failed to download {bin_filename} from {fallback_server}: {e}"
                    )
                    self.config_manager.set_config_value("is_online", False)

        threading.Thread(target=task, daemon=True).start()

    def flash_local_bin(self, local_filepath):
        """Flash a local BIN file."""
        def task():
            try:
                if not os.path.isfile(local_filepath):
                    raise FileNotFoundError(f"Local file not found: {local_filepath}")
                self.logger.terminal_print(f"Using local bin: {local_filepath}")
                self.flash_firmware(local_filepath)
            except Exception as e:
                self.logger.terminal_print(f"Failed to flash local bin: {e}")
        threading.Thread(target=task, daemon=True).start()

    def flash_firmware(self, bin_path):
        """Start the flashing process in a separate thread."""
        if not bin_path:
            self.logger.terminal_print("No BIN file specified for flashing.")
            return
        try:
            self.logger.terminal_print("Starting flashing...\n")
            threading.Thread(target=self.flash_firmware_thread, args=(bin_path,), daemon=True).start()
        except Exception as e:
            self.logger.terminal_print(f"Error initiating flashing: {e}")

    def flash_firmware_thread(self, bin_path):
        """Handle the flashing process."""
        if not os.path.isfile(bin_path):
            self.logger.terminal_print(f"BIN file does not exist: {bin_path}")
            return

        # Ensure flashing doesn't run concurrently
        with self.flashing_lock:
            self.is_flashing = True
            self.serial_handler.set_flashing(True)
            self.logger.terminal_print(f"Loaded file: {bin_path}")

            if self.serial_handler.is_connected:
                self.serial_handler.stop_monitoring()
                if self.serial_handler.monitoring_thread and self.serial_handler.monitoring_thread.is_alive():
                    self.serial_handler.monitoring_thread.join(timeout=5)

            time.sleep(0.5)

            process_failed = False
            process = None
            success_detected = False
            bootloader_warning_detected = False

            try:
                esptool_args = self.esptool_cmd + [
                    '--chip', 'esp32s3',
                    '--port', self.serial_handler.com_port,
                    '--baud', '921600',
                    'write_flash', '0x0', bin_path
                ]

                if sys.platform.startswith('win'):
                    startupinfo = subprocess.STARTUPINFO()
                    startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                    process = subprocess.Popen(
                        esptool_args,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                        shell=False,
                        startupinfo=startupinfo
                    )
                else:
                    process = subprocess.Popen(
                        esptool_args,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                        shell=False
                    )

                def read_stream(pipe, stream_name):
                    nonlocal success_detected, bootloader_warning_detected
                    for line in iter(pipe.readline, ''):
                        if "Writing at" in line:
                            self.logger.terminal_print(line.strip())
                        if "Hash of data verified." in line:
                            success_detected = True
                        if "Leaving... WARNING: ESP32-S3" in line:
                            bootloader_warning_detected = True
                    pipe.close()

                stdout_thread = threading.Thread(target=read_stream, args=(process.stdout, 'STDOUT'), daemon=True)
                stderr_thread = threading.Thread(target=read_stream, args=(process.stderr, 'STDERR'), daemon=True)
                stdout_thread.start()
                stderr_thread.start()

                process.wait()

                stdout_thread.join()
                stderr_thread.join()

                if process.returncode == 0 or success_detected or bootloader_warning_detected:
                    self.logger.terminal_print("\nFlashing completed successfully.\n\n Please remove the usb cable\n\n Thanks for using MAKCU.")
                else:
                    process_failed = True

            except Exception as e:
                self.logger.terminal_print(f"Flashing error: {e}")
                process_failed = True

            finally:
                if process and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                if process_failed and not bootloader_warning_detected:
                    self.logger.terminal_print("Flashing encountered errors.")
                self.is_flashing = False
                self.serial_handler.set_flashing(False)
                self.serial_handler.start_monitoring()
