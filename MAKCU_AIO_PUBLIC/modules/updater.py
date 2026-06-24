import requests
import threading
import subprocess
import os
import time
import sys
import hashlib
import json
import re

IS_WINDOWS = sys.platform.startswith("win")

from modules.utils import get_main_folder


class Updater:
    """
    Handles checking for updates and applying them.
    Uses ConfigManager for online status and server prioritization.
    Firmware details are provided via ConfigManager's ``firmware`` entries,
    each containing ``name``, ``version``, ``changelog``, ``primary_url``,
    and ``fallback_url`` for both ``left`` and ``right`` sides.
    """
    PRIMARY_UPDATE_BASE_URL = "https://github.com/terrafirma2021/MAKCM_v2_files/raw/refs/heads/main/MAKCU.exe"
    FALLBACK_UPDATE_BASE_URL = "https://gitee.com/terrafirma/MAKCM_v2_files/raw/main/MAKCU.exe"
    def __init__(self, logger, config_manager, flasher=None):
        self.logger = logger
        self.config_manager = config_manager
        self.flasher = flasher
        self.main_folder = get_main_folder()
        self.current_version = self._get_current_version()
        self.update_check_complete = threading.Event()
        self.is_offline = False

    def _get_current_version(self):
        """Determine the version of the currently running build.

        Order of precedence:
        1. The ``version`` field from the locally bundled ``config.json``.
           This file is packaged with the executable and reflects the build
           version at creation time.
        2. The version embedded in the executable name following the pattern
           ``MAKCU_<major>_<minor>.exe``.
        3. Fallback to ``"0.0"`` if no information is available.

        Future releases should ensure either the bundled ``config.json`` has a
        correct ``version`` field or the executable name encodes the version so
        this method can resolve the running version reliably.
        """

        # 1. Try the local config.json
        config_path = os.path.join(self.main_folder, "config.json")
        try:
            with open(config_path, "r", encoding="utf-8") as f:
                data = json.load(f)
                version = str(data.get("version", "")).strip()
                if version:
                    return version
        except Exception:
            pass

        # 2. Try to parse from executable name
        exe_name = os.path.basename(sys.argv[0])
        match = re.search(r"_(\d+_\d+)\.exe$", exe_name, re.IGNORECASE)
        if match:
            return match.group(1).replace("_", ".")

        # 3. Unknown version
        return "0.0"

    def check_for_updates(self):
        """
        Checks if there is a newer version in config.json; if so, downloads and
        launches the new .exe from the primary or fallback URL. Always prints the main version changelog.
        Uses ConfigManager's last_successful_server for URL prioritization.
        """
        def task():
            try:
                # Get currently bundled firmware versions before refresh
                current_fw = self.config_manager.get_config_value("firmware", {})
                current_firmware_left = current_fw.get("left", {}).get("version", "")
                current_firmware_right = current_fw.get("right", {}).get("version", "")

                # Ensure config is downloaded before any checks
                self.config_manager.wait_until_downloaded()

                # Check if offline via ConfigManager
                if not self.config_manager.is_online_status():
                    self.logger.terminal_print("Offline mode detected via ConfigManager. Skipping update checks.")
                    self.is_offline = True
                    self.update_check_complete.set()
                    return

                # Retrieve latest version and firmware from config
                latest_version = self.config_manager.get_config_value("version")
                main_changelog = self.config_manager.get_config_value("main_aio_changelog", [])
                left_info = self.config_manager.get_firmware_info("left") or {}
                right_info = self.config_manager.get_firmware_info("right") or {}

                if not latest_version:
                    self.logger.terminal_print("Latest version not specified in configuration.")
                    self.update_check_complete.set()
                    return
        
                current_version = self.current_version

                # Log version details for debugging
                self.logger.terminal_print(f"Current version: {current_version!r}  Latest version: {latest_version!r}")

                # Always print main version changelog
                self.logger.terminal_print("\n*** Main Version Changelog ***")
                for item in main_changelog:
                    for change in item.get("changes", []):
                        self.logger.terminal_print(f"- {change}")
                self.logger.terminal_print("\n")

                # Check firmware versions
                firmware_update_needed = False

                if left_info and self.is_different_version(left_info.get("version", ""), current_firmware_left):
                    self.logger.terminal_print("\n*** Left firmware is available ***")
                    self.logger.terminal_print(f"Version: {left_info.get('version', 'Unknown')}")
                    self.logger.terminal_print(f"File: {left_info['filename']}")
                    self.logger.terminal_print("Changelog:")
                    for change in left_info.get("changelog", []):
                        self.logger.terminal_print(f"- {change}\n")
                    firmware_update_needed = True
                    if self.flasher:
                        self.flasher.download_and_flash("left")

                if right_info and self.is_different_version(right_info.get("version", ""), current_firmware_right):
                    self.logger.terminal_print("\n*** Right firmware is available ***")
                    self.logger.terminal_print(f"Version: {right_info.get('version', 'Unknown')}")
                    self.logger.terminal_print(f"File: {right_info['filename']}")
                    self.logger.terminal_print("Changelog:")
                    for change in right_info.get("changelog", []):
                        self.logger.terminal_print(f"- {change}\n")
                    firmware_update_needed = True
                    if self.flasher:
                        self.flasher.download_and_flash("right")

                if not firmware_update_needed:
                    self.logger.terminal_print("You are up to date with firmware.\n")

                # Check if the software version is different
                if self.is_different_version(latest_version, current_version):
                    if not IS_WINDOWS:
                        self.logger.terminal_print(
                            f"New AIO version {latest_version} available. In-app update "
                            "is Windows-only; download the latest release manually."
                        )
                        return
                    self.logger.terminal_print("New version available. Downloading update...")

                    new_exe_name = f"MAKCU_{latest_version.replace('.', '_')}.exe"
                    new_exe_path = os.path.join(self.main_folder, new_exe_name)

                    # Log the main folder and new executable path for debugging
                    self.logger.terminal_print(f"Main folder: {self.main_folder}")
                    self.logger.terminal_print(f"New executable path: {new_exe_path}")

                    # Ensure the target file is overwritten if it exists
                    if os.path.exists(new_exe_path):
                        self.logger.terminal_print(f"Existing versioned file found. Removing: {new_exe_path}")
                        os.remove(new_exe_path)

                    # Determine which URL to try first based on last successful server
                    primary_url = self.PRIMARY_UPDATE_BASE_URL
                    fallback_url = self.FALLBACK_UPDATE_BASE_URL
                    primary_server = "GitHub"
                    fallback_server = "Gitee"
                    last_successful_server = self.config_manager.get_config_value("last_successful_server")
                    self.logger.terminal_print(f"Last successful server: {last_successful_server or 'None'}")
                    if last_successful_server == "gitee":
                        primary_url, fallback_url = fallback_url, primary_url
                        primary_server, fallback_server = fallback_server, primary_server
                        self.logger.terminal_print(f"Prioritizing {primary_server} URL: {primary_url}")
                    else:
                        self.logger.terminal_print(f"Prioritizing {primary_server} URL: {primary_url}")

                    # Download the executable
                    self.logger.terminal_print(f"Attempting download from primary URL: {primary_url}")
                    downloaded = self.download_file(primary_url, new_exe_path, primary_server)
                    connected_server = primary_server if downloaded else None
                    if not downloaded:
                        self.logger.terminal_print(f"Primary download failed. Trying fallback URL: {fallback_url}")
                        downloaded = self.download_file(fallback_url, new_exe_path, fallback_server)
                        connected_server = fallback_server if downloaded else None

                    if downloaded:
                        # Verify the file exists and is not a directory before launching
                        if os.path.exists(new_exe_path) and os.path.isfile(new_exe_path):
                            self.logger.terminal_print(f"Successfully connected to {connected_server} server for update.")
                            self.logger.terminal_print(f"Launching new version: {new_exe_name}")
                            normalized_path = os.path.normpath(new_exe_path)
                            self.logger.terminal_print(f"Normalized path: {normalized_path}")

                            # Verify executable permissions
                            try:
                                os.access(normalized_path, os.X_OK)
                                self.logger.terminal_print(f"Executable permissions verified for: {normalized_path}")
                            except Exception as e:
                                self.logger.terminal_print(f"Warning: Failed to verify executable permissions: {e}")

                            # Launch the new executable
                            try:
                                # Use shell=False and pass the path as a single argument
                                self.logger.terminal_print(f"Executing command: {normalized_path}")
                                subprocess.Popen([normalized_path], shell=False)
                                time.sleep(0.2)  # Delay before shutting down this version
                                self.logger.terminal_print("Exiting current version.")
                                os._exit(0)
                            except Exception as e:
                                self.logger.terminal_print(f"Failed to launch new version: {e}")
                                self.logger.terminal_print("Running current version.")
                        else:
                            self.logger.terminal_print(f"Error: {new_exe_path} does not exist or is not a file. Cannot launch update.")
                    else:
                        self.logger.terminal_print("Failed to download update from both servers. Running current version.")
                        self.is_offline = True
                        self.config_manager.set_config_value("is_online", False)

                else:
                    self.logger.terminal_print("Software is up to date.\n")

            except Exception as e:
                self.logger.terminal_print(f"Update check failed: {e}")
                self.is_offline = True
                self.config_manager.set_config_value("is_online", False)
            finally:
                self.update_check_complete.set()

        threading.Thread(target=task, daemon=True).start()

    def is_different_version(self, latest, current):
        """
        Return True if the latest version is different from the current version.
        """
        try:
            latest_clean = str(latest).strip()
            current_clean = str(current).strip()
            return latest_clean != current_clean
        except Exception:
            return False


    def download_file(self, url, destination, server_name):
        """
        Downloads a file from a URL and saves it directly to the destination.
        Returns True if successful, False otherwise.
        Tracks the server used for a successful download and logs download progress.
        """
        try:
            self.logger.terminal_print(f"Starting download from {server_name} server: {url}")
            response = requests.get(url, stream=True, timeout=30)
            response.raise_for_status()

            # Get total file size for progress tracking
            total_size = int(response.headers.get('content-length', 0))
            self.logger.terminal_print(f"File size: {total_size} bytes")
            downloaded_size = 0
            chunk_size = 8192

            with open(destination, 'wb') as f:
                start_time = time.time()
                for chunk in response.iter_content(chunk_size=chunk_size):
                    if chunk:
                        f.write(chunk)
                        downloaded_size += len(chunk)
                        # Log progress every ~10% or at completion
                        if total_size > 0:
                            progress = (downloaded_size / total_size) * 100
                            if downloaded_size == total_size or (downloaded_size % (total_size // 10) < chunk_size):
                                self.logger.terminal_print(f"Download progress: {progress:.1f}% ({downloaded_size}/{total_size} bytes)")
                        else:
                            self.logger.terminal_print(f"Downloaded chunk: {downloaded_size} bytes")

            end_time = time.time()
            self.logger.terminal_print(f"Download completed in {end_time - start_time:.2f} seconds")
            self.logger.terminal_print(f"File saved to: {destination}")

            # Verify file integrity with size and a simple hash
            if os.path.exists(destination):
                file_size = os.path.getsize(destination)
                self.logger.terminal_print(f"Downloaded file size: {file_size} bytes")
                if file_size == 0:
                    self.logger.terminal_print("Error: Downloaded file is empty")
                    return False
                with open(destination, 'rb') as f:
                    file_hash = hashlib.md5(f.read()).hexdigest()
                    self.logger.terminal_print(f"File MD5 hash: {file_hash}")
            else:
                self.logger.terminal_print(f"Error: File not found at {destination}")
                return False

            # Update last successful server
            self.config_manager.set_config_value("last_successful_server", "github" if "github" in url.lower() else "gitee")
            self.config_manager.set_config_value("is_online", True)
            return True
        except Exception as e:
            self.logger.terminal_print(f"Failed to download from {server_name} server: {e}")
            return False