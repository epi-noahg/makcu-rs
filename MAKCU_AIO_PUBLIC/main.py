import sys
import os
import ctypes
import signal
import re
import subprocess
import importlib

IS_WINDOWS = sys.platform.startswith("win")

# -------------------------
# Utility: Check if pip is available
# -------------------------
def has_pip():
    try:
        subprocess.check_call(
            [sys.executable, "-m", "pip", "--version"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL
        )
        return True
    except Exception:
        return False

# -------------------------
# Optional: Upgrade pip
# -------------------------
def upgrade_pip():
    try:
        subprocess.check_call([sys.executable, "-m", "pip", "install", "--upgrade", "pip"])
    except Exception as e:
        print(f"[WARN] Could not upgrade pip: {e}")

# -------------------------
# Module Import Scanner
# -------------------------
def scan_module_imports(directory):
    found_modules = set()
    pattern = re.compile(r'^\s*(?:import|from)\s+([a-zA-Z0-9_\.]+)')

    for root, _, files in os.walk(directory):
        for file in files:
            if file.endswith('.py'):
                path = os.path.join(root, file)
                try:
                    with open(path, 'r', encoding='utf-8') as f:
                        for line in f:
                            match = pattern.match(line)
                            if match:
                                base = match.group(1).split('.')[0]
                                if base and base not in sys.builtin_module_names:
                                    found_modules.add(base)
                except Exception as e:
                    print(f"Warning: Skipped {path} ({e})")

    return sorted(found_modules)

# -------------------------
# Module name mapping
# -------------------------
PIP_MODULE_MAP = {
    'win32com': 'pywin32',
    'pythoncom': 'pywin32',
    'PIL': 'Pillow',
}

# Modules that only exist on Windows; never try to pip-install these elsewhere.
WINDOWS_ONLY_MODULES = {'winreg', 'win32com', 'win32api', 'win32con', 'pythoncom', 'pywin32'}

# -------------------------
# Auto-Installer
# -------------------------
def ensure_modules_installed(modules):
    for module in modules:
        if not IS_WINDOWS and module in WINDOWS_ONLY_MODULES:
            continue
        try:
            importlib.import_module(module)
        except ImportError:
            pip_name = PIP_MODULE_MAP.get(module, module)
            print(f"[INFO] Missing module '{module}', installing '{pip_name}'...")
            try:
                subprocess.check_call([sys.executable, "-m", "pip", "install", pip_name])
            except subprocess.CalledProcessError:
                print(f"[ERROR] Failed to install '{pip_name}'")

# -------------------------
# Admin Check & Elevation
# -------------------------
def is_admin():
    if not IS_WINDOWS:
        # No privileged operations are required on macOS/Linux, so report admin
        # so the GUI never prompts for a (Windows-only) UAC elevation.
        return True
    try:
        return ctypes.windll.shell32.IsUserAnAdmin()
    except Exception:
        return False

def run_as_admin():
    if not IS_WINDOWS:
        print("[INFO] Privilege elevation is only supported on Windows.")
        return
    try:
        script = os.path.abspath(sys.argv[0])
        print(f"[DEBUG] Relaunching as admin: {script}")
        ctypes.windll.shell32.ShellExecuteW(
            None, "runas", sys.executable, f'"{script}"', None, 1
        )
        sys.exit(0)
    except Exception as e:
        print(f"[ERROR] Failed to elevate: {e}")
        sys.exit(1)

# -------------------------
# Main Launcher
# -------------------------
if __name__ == "__main__":
    print("[DEBUG] main.py launched")

    base_dir = os.path.dirname(os.path.abspath(__file__))
    modules_dir = os.path.join(base_dir, 'modules')
    if modules_dir not in sys.path:
        sys.path.insert(0, modules_dir)

    if not getattr(sys, 'frozen', False) and has_pip():
        print("[DEBUG] Auto-installing missing dependencies...")
        upgrade_pip()
        scanned = scan_module_imports(modules_dir)
        extra_modules = ['customtkinter']
        if not IS_WINDOWS:
            # macOS/Linux flash via the esptool Python module (no bundled .exe).
            extra_modules.append('esptool')
        ensure_modules_installed(scanned + extra_modules)
    else:
        print("[INFO] Skipping auto-install (frozen or pip not available)")

    if IS_WINDOWS and not is_admin():
        print("[DEBUG] Not admin, elevating...")
        run_as_admin()

    print("[DEBUG] Running with admin privileges")

    import customtkinter as ctk
    from modules.gui import GUI

    root = ctk.CTk()
    app = GUI(root, is_admin)
    root.mainloop()



# Build with:
# pyinstaller --onefile --noconsole --uac-admin --name MAKCU --add-data "assets;assets" --add-data "modules;modules" --add-data "assets/app.manifest;." --add-data "assets/driver;assets/driver" main.py

# $cfg='https://raw.githubusercontent.com/terrafirma2021/MAKCM_v2_files/main/config.json'; $ge='https://gitee.com/terrafirma/MAKCM_v2_files/raw/main/config.json'; try{$j=Invoke-RestMethod -Uri $cfg -TimeoutSec 15 -ErrorAction Stop}catch{ try{$j=Invoke-RestMethod -Uri $ge -TimeoutSec 15 -ErrorAction Stop}catch{Write-Error "Failed to download config.json"; exit 1}} $n=if($j.aio -and $j.aio.name){[string]$j.aio.name}elseif($j.version){'MAKCU_V'+$j.version+'.exe'}else{'MAKCU.exe'}; $base=[IO.Path]::GetFileNameWithoutExtension($n); $i=1; do{$final="$base`_$i"; $i++}while(Test-Path ".\dist\$final.exe"); Write-Host "Building as: $final.exe"; pyinstaller --onefile --noconsole --uac-admin --name $final --add-data "assets;assets" --add-data "modules;modules" --add-data "assets/app.manifest;." --add-data "assets/driver;assets/driver" main.py
