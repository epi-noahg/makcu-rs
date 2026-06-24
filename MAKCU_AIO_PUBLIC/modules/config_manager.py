# config_manager.py

import requests
import threading
import os
import json
import aiohttp
import asyncio
import time
from tenacity import retry, stop_after_attempt, wait_fixed
from aiohttp import ContentTypeError
from ping3 import ping
from modules.utils import get_main_folder, get_download_path


class ConfigManager:
    """
    Downloads/stores configuration data and firmware BINs.
    Also exposes AIO (MAKCU.exe) info via get_aio_info().

    JSON expectations:
      - "version": "2.7"                   (global)
      - "aio": {
            "version": "2.7",
            "name": "MAKCU_V2.7.exe",
            "primary_url": "https://.../MAKCU_V2.7.exe",
            "fallback_url": "https://.../MAKCU_V2.7.exe",
            "changelog": [ ... ]
        }
      - "firmware": {
            "left":  { "version": "3.6", "name": "V3.6_LEFT",  "primary_url": "...", "fallback_url":"..." },
            "right": { "version": "3.6", "name": "V3.6_RIGHT", "primary_url": "...", "fallback_url":"..." }
        }
    """
    PRIMARY_CONFIG_URL = "https://raw.githubusercontent.com/terrafirma2021/MAKCM_v2_files/main/config.json"
    FALLBACK_CONFIG_URL = "https://gitee.com/terrafirma/MAKCM_v2_files/raw/main/config.json"
    LOCAL_CONFIG_PATH = os.path.join(get_main_folder(), 'config.json')
    PING_TIMEOUT = 0.5  # 500ms

    def __init__(self, logger, progress_callback=None):
        self.logger = logger
        self.progress_callback = progress_callback
        self.config_data = {}
        self.config_lock = threading.Lock()
        self.download_complete = threading.Event()
        self.download_successful = False
        self.is_online = False
        self.bin_file_urls = {}
        self.side_to_filename = {}
        self.bin_files_downloaded = {}
        self.preferred_server = self._select_fastest_server()
        self.load_local_config()
        self._parse_firmware_info()
        threading.Thread(target=self.download_all_files, daemon=True).start()

    @staticmethod
    def _parse_ping(raw) -> float | None:
        if raw is None:
            return None
        if isinstance(raw, (int, float)):
            return float(raw)
        if isinstance(raw, str):
            s = raw.strip().lower()
            factor = 1.0
            if s.endswith("ms"):
                s = s[:-2].strip()
                factor = 1 / 1000
            elif s.endswith("s"):
                s = s[:-1].strip()
            try:
                return float(s) * factor
            except ValueError:
                return None
        return None

    def _select_fastest_server(self):
        servers = [("GitHub", "raw.githubusercontent.com"), ("Gitee", "gitee.com")]
        results = {}

        def safe_ping(name, host):
            try:
                latency = self._parse_ping(ping(host, timeout=self.PING_TIMEOUT))
                results[name] = latency if latency is not None else -1.0
            except Exception:
                results[name] = -1.0

        threads = [threading.Thread(target=safe_ping, args=(n, h), daemon=True) for n, h in servers]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        valid = {k: v for k, v in results.items() if v >= 0}
        fastest = min(valid, key=valid.get) if valid else "GitHub"

        def fmt(name):
            v = results.get(name, -1)
            return f"{v * 1000:.2f}ms" if v >= 0 else "failed"

        self.logger.terminal_print(f"Connected to GitHub: {fmt('GitHub')}  Gitee: {fmt('Gitee')}")
        return fastest

    def load_local_config(self):
        if os.path.exists(self.LOCAL_CONFIG_PATH):
            try:
                with open(self.LOCAL_CONFIG_PATH, 'r', encoding="utf-8") as f:
                    with self.config_lock:
                        self.config_data = json.load(f)
                        self.download_successful = True
                        self.is_online = self.config_data.get("is_online", False)
            except Exception:
                pass

    def _parse_firmware_info(self):
        firmware = self.config_data.get("firmware", {}) or {}
        # reset each time so stale filenames (e.g., V3.2) don't linger
        self.bin_file_urls = {}
        self.side_to_filename = {}
        self.bin_files_downloaded = {}
        for side, info in firmware.items():
            info = info or {}
            primary_url = info.get("primary_url")
            fallback_url = info.get("fallback_url")
            if not primary_url or not fallback_url:
                continue
            base = info.get("name")
            if not base or not str(base).strip():
                base = os.path.basename(primary_url)
            filename = base if str(base).lower().endswith(".bin") else f"{base}.bin"
            self.bin_file_urls[filename] = {"primary": primary_url, "fallback": fallback_url}
            self.side_to_filename[side] = filename
            self.bin_files_downloaded[filename] = self._is_valid_file(get_download_path(filename))

    def _is_valid_file(self, filepath):
        return os.path.exists(filepath) and os.path.getsize(filepath) > 0

    async def download_file_async(self, session, filename, primary_url, fallback_url):
        bin_path = get_download_path(filename)
        os.makedirs(os.path.dirname(bin_path), exist_ok=True)

        if self._is_valid_file(bin_path):
            if self.progress_callback:
                self.progress_callback(filename, "skipped")
            return self.config_data.get("last_successful_server", "github")

        @retry(stop=stop_after_attempt(3), wait=wait_fixed(2))
        async def fetch(url):
            async with session.get(url, timeout=10) as resp:
                resp.raise_for_status()
                with open(bin_path, 'wb') as f:
                    async for chunk in resp.content.iter_chunked(8192):
                        f.write(chunk)
            return True

        urls = [(primary_url, "GitHub"), (fallback_url, "Gitee")]
        if self.preferred_server == "Gitee":
            urls.reverse()

        for url, server in urls:
            try:
                await fetch(url)
                if self.progress_callback:
                    self.progress_callback(filename, "success")
                return server.lower()
            except Exception:
                if self.progress_callback:
                    self.progress_callback(filename, "failed")
        return None

    async def download_all_files_async(self):
        start_time = time.time()
        if self.progress_callback:
            self.progress_callback("all", "starting")

        async with aiohttp.ClientSession() as session:
            @retry(stop=stop_after_attempt(3), wait=wait_fixed(2))
            async def fetch_config(url):
                async with session.get(url, timeout=10) as resp:
                    resp.raise_for_status()
                    # Accept JSON even if served as text/plain
                    try:
                        _ = resp.content_type  # touch for clarity
                    except Exception:
                        pass
                    text = await resp.text(encoding='utf-8')
                    return json.loads(text)

            config_urls = [
                (self.PRIMARY_CONFIG_URL, "GitHub"),
                (self.FALLBACK_CONFIG_URL, "Gitee")
            ]
            if self.preferred_server == "Gitee":
                config_urls.reverse()

            fetched = False
            for url, server in config_urls:
                try:
                    new_cfg = await fetch_config(url)
                    with self.config_lock:
                        keep_window = self.config_data.get("window_position")
                        self.config_data = new_cfg or {}
                        if keep_window:
                            self.config_data["window_position"] = keep_window
                        self.download_successful = True
                        self.is_online = True
                        self.config_data["is_online"] = True
                        self.config_data["last_successful_server"] = server.lower()
                        self._parse_firmware_info()
                        self.save_config_to_file()
                    if self.progress_callback:
                        self.progress_callback("config.json", "success")
                    fetched = True
                    break
                except Exception:
                    continue

            if not fetched:
                with self.config_lock:
                    self.is_online = False
                    self.config_data["is_online"] = False
                    self._parse_firmware_info()
                    self.save_config_to_file()
                if self.progress_callback:
                    self.progress_callback("config.json", "failed")

            # Debug-plan log of what we’ll download
            try:
                with self.config_lock:
                    dbg_files = list(self.bin_file_urls.keys())
                if self.progress_callback:
                    for fn in dbg_files:
                        self.progress_callback(f"plan:{fn}", "info")
            except Exception:
                pass

            # Firmware bin downloads
            tasks = []
            for filename, urls in self.bin_file_urls.items():
                tasks.append(self.download_file_async(session, filename, urls["primary"], urls["fallback"]))

            results = await asyncio.gather(*tasks, return_exceptions=True)
            with self.config_lock:
                for filename, result in zip(self.bin_file_urls.keys(), results):
                    ok = isinstance(result, str)
                    self.bin_files_downloaded[filename] = ok
                    if ok:
                        self.config_data["last_successful_server"] = result
                self.is_online = any(self.bin_files_downloaded.values()) or self.download_successful
                self.config_data["is_online"] = self.is_online
                self.save_config_to_file()

        if self.progress_callback:
            self.progress_callback("all", "complete")

    def download_all_files(self):
        asyncio.run(self.download_all_files_async())
        self.download_complete.set()

    def save_config_to_file(self):
        try:
            config_dir = os.path.dirname(self.LOCAL_CONFIG_PATH)
            os.makedirs(config_dir, exist_ok=True)
            with open(self.LOCAL_CONFIG_PATH, 'w', encoding="utf-8") as f:
                json.dump(self.config_data, f, indent=4)
        except Exception:
            pass

    def get_config_value(self, key, default=None):
        with self.config_lock:
            return self.config_data.get(key, default)

    def set_config_value(self, key, value):
        with self.config_lock:
            self.config_data[key] = value
            self.save_config_to_file()

    def wait_until_downloaded(self, timeout=30):
        return self.download_complete.wait(timeout)

    def is_online_status(self):
        with self.config_lock:
            return self.is_online

    def get_firmware_info(self, side):
        filename = self.side_to_filename.get(side)
        if not filename:
            return None
        urls = self.bin_file_urls.get(filename, {})
        with self.config_lock:
            fw_entry = (self.config_data.get("firmware", {}) or {}).get(side, {}) or {}
        version = str(fw_entry.get("version", "")).strip()
        name = fw_entry.get("name")
        if not name or not str(name).strip():
            name = filename[:-4] if filename.lower().endswith(".bin") else filename
        return {
            "name": name,
            "filename": filename,
            "primary_url": urls.get("primary"),
            "fallback_url": urls.get("fallback"),
            "version": version,
            "changelog": fw_entry.get("changelog", []),
        }

    def get_firmware_urls(self, side):
        info = self.get_firmware_info(side)
        if not info:
            return None, None
        return info["primary_url"], info["fallback_url"]

    def is_bin_downloaded(self, name):
        filename = f"{name}.bin" if not name.endswith('.bin') else name
        bin_path = get_download_path(filename)
        return self._is_valid_file(bin_path) and self.bin_files_downloaded.get(filename, False)

    def get_aio_info(self):
        """
        Return dict with AIO fields: version, name, primary_url, fallback_url, changelog[].
        Falls back to global version if aio.version is missing.
        """
        with self.config_lock:
            aio = (self.config_data.get("aio") or {}).copy()
            global_version = str(self.config_data.get("version", "")).strip()
        # Normalize
        aio["version"] = str(aio.get("version", "") or global_version).strip()
        aio["name"] = str(aio.get("name", "")).strip()
        aio["primary_url"] = str(aio.get("primary_url", "")).strip()
        aio["fallback_url"] = str(aio.get("fallback_url", "")).strip()
        aio["changelog"] = list(aio.get("changelog", []) or [])
        return aio
