# modules/utils.py

import os
import sys
from pathlib import Path

# ─────────────────────────────────────────────────────────────────────────────
# PyInstaller-safe path helpers
#  - NO copying from sys._MEIPASS to a temp folder (slow & fragile).
#  - Read bundled resources directly from the unpack dir (sys._MEIPASS).
#  - Write/download only next to the EXE (or project root in dev).
# ─────────────────────────────────────────────────────────────────────────────

def _is_frozen() -> bool:
    """True when running under PyInstaller onefile/onedir."""
    return bool(getattr(sys, "frozen", False)) and hasattr(sys, "_MEIPASS")


def app_dir() -> Path:
    """
    Directory where we want to WRITE things (logs, downloads, etc.).
    - Frozen: folder containing the EXE.
    - Dev:    project root (2 levels up from this file: modules/utils.py).
    """
    if _is_frozen():
        return Path(sys.executable).resolve().parent
    # Adjust if your repo layout differs; this assumes modules/utils.py
    return Path(__file__).resolve().parent.parent


def bundle_dir() -> Path:
    """
    Directory where READ-only bundled resources live.
    - Frozen: PyInstaller unpack dir (sys._MEIPASS).
    - Dev:    project root (same as app_dir()).
    """
    if _is_frozen():
        return Path(sys._MEIPASS)  # nosec - runtime extraction dir
    return app_dir()


def get_main_folder() -> str:
    """
    Kept for backward compatibility with existing calls.
    Returns the folder we consider the app base (where we write files).
    """
    return str(app_dir())


def ensure_dir(p: Path) -> None:
    """Create parent directory for a file path, if needed."""
    p.parent.mkdir(parents=True, exist_ok=True)


def resource_path(rel: str) -> str:
    """
    Build an absolute path to a bundled resource that was included via --add-data.
    Example:
        icon = resource_path("assets/icons/app.ico")
        driver = resource_path("assets/driver/CH343S64.SYS")
    """
    abs_path = (bundle_dir() / rel).resolve()
    if not abs_path.exists():
        # Helpful error with both attempted location and frozen status
        raise FileNotFoundError(
            f"Bundled resource not found: {abs_path}\n"
            f"(frozen={_is_frozen()}, bundle_dir={bundle_dir()})"
        )
    return str(abs_path)


def get_download_path(filename: str) -> str:
    """
    Place downloaded/created files alongside the EXE (or project root in dev),
    under a dedicated 'downloads' folder.
    """
    target = (app_dir() / "downloads" / filename).resolve()
    ensure_dir(target)
    return str(target)


# ─────────────────────────────────────────────────────────────────────────────
# Convenience wrappers used throughout the codebase
# ─────────────────────────────────────────────────────────────────────────────

def get_icon_path(filename: str) -> str:
    """
    Return full path to an icon or any file inside /assets when bundled.
    Example: get_icon_path("app.ico") -> <bundle>/assets/app.ico
    """
    return resource_path(str(Path("assets") / filename))


def get_driver_path(filename: str | None = None) -> str:
    """
    Return full path to driver directory or a specific driver file inside /assets/driver.
    Example:
        get_driver_path() -> <bundle>/assets/driver
        get_driver_path("CH343S64.SYS") -> <bundle>/assets/driver/CH343S64.SYS
    """
    base = Path("assets") / "driver"
    if filename:
        return resource_path(str(base / filename))
    # Return the directory path (ensure it exists)
    dir_path = (bundle_dir() / base).resolve()
    if not dir_path.exists():
        raise FileNotFoundError(
            f"Driver folder not found: {dir_path}\n"
            f"(frozen={_is_frozen()}, bundle_dir={bundle_dir()})"
        )
    return str(dir_path)


# ─────────────────────────────────────────────────────────────────────────────
# Backward-compat shims (if something still imports these names)
# ─────────────────────────────────────────────────────────────────────────────

def setup_custom_temp_folder() -> str:
    """
    Deprecated: copying sys._MEIPASS content into a custom temp dir is unnecessary.
    Kept only to avoid crashes if older code calls it. Returns bundle_dir().
    """
    return str(bundle_dir())
