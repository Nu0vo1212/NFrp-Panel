#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
FrpPanel 桌面控制台
===================

一个只用 Python 标准库（Tkinter）的小工具，用来管理本项目：

  * 一键启动 / 停止 / 重启 FrpPanel 后端（cargo run）和前端（Vite）
  * 自动发现已在运行的实例（例如 dev.bat 启动的），不会重复启动
  * 实时显示后端 / 前端日志，支持清空、复制、自动滚动
  * 打开面板、查看初始管理员口令
  * 忘记密码时直接重置管理员登录密码：不需要旧密码，直接写入本地
    SQLite（后端运行中也能生效），不在本地保存明文密码

用法：
    python panel_gui.py
    python panel_gui.py --autostart     # 启动后自动启动服务（调试用）
    python panel_gui.py --selftest      # 不开窗口，检查环境与接口

注意：默认绑定 127.0.0.1，只有本机可访问；端口被占用时会自动换一个并写日志。
"""

from __future__ import annotations

import ctypes
import json
import os
import queue
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import tkinter as tk
import urllib.request
import webbrowser
from pathlib import Path
from tkinter import messagebox, ttk

# ---------------------------------------------------------------------------
# 常量
# ---------------------------------------------------------------------------
APP_NAME = "FrpPanel 控制台"
APP_VERSION = "1.0.0"
ROOT = Path(__file__).resolve().parent
FRONTEND_DIR = ROOT / "frontend"
SETTINGS_FILE = ROOT / "run" / "gui-settings.json"

BACKEND_PORT_DEFAULT = 8787
FRONTEND_PORT_DEFAULT = 5173
BACKEND_PORT_ALTS = (8899, 8788, 8790, 18080, 18787)
FRONTEND_PORT_ALTS = (5174, 5175, 4173, 5180)

# 配色与 UI 规范保持一致（浅色 + 克制）
BG = "#F5F6F8"
CARD = "#FFFFFF"
BORDER = "#E3E6EB"
TEXT = "#1F2328"
MUTED = "#5B616E"
FAINT = "#8A909B"
ACCENT = "#3B82F6"
ACCENT_DARK = "#2563EB"
SUCCESS = "#16A34A"
WARNING = "#D97706"
DANGER = "#DC2626"
LOG_BG = "#14161A"
LOG_FG = "#D7DEE8"

FONT_UI = "Microsoft YaHei UI"
FONT_MONO = "Consolas"

CREATE_NO_WINDOW = 0x08000000 if os.name == "nt" else 0
ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")


# ---------------------------------------------------------------------------
# 基础工具
# ---------------------------------------------------------------------------
def strip_ansi(text: str) -> str:
    return ANSI_RE.sub("", text)


def port_in_use(port: int, host: str = "127.0.0.1") -> bool:
    try:
        with socket.create_connection((host, port), timeout=0.25):
            return True
    except OSError:
        return False


def pick_port(preferred: int, alternatives: tuple[int, ...]) -> tuple[int, bool]:
    """返回 (可用端口, 是否改过端口)。"""
    if not port_in_use(preferred):
        return preferred, False
    for candidate in alternatives:
        if candidate != preferred and not port_in_use(candidate):
            return candidate, True
    raise RuntimeError(f"端口 {preferred} 被占用，且备用端口都不可用，请手动填写一个空闲端口。")


def http_json(
    url: str,
    method: str = "GET",
    payload: dict | None = None,
    token: str | None = None,
    timeout: float = 6.0,
) -> dict:
    data = None
    if payload is not None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Accept", "application/json")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read()
    if not raw:
        return {}
    return json.loads(raw.decode("utf-8", "replace"))


def probe_backend(port: int, timeout: float = 1.5) -> dict | None:
    try:
        data = http_json(f"http://127.0.0.1:{port}/api/health", timeout=timeout)
        if data.get("status") == "ok":
            return {"port": port, "version": str(data.get("version", "?"))}
    except Exception:
        pass
    return None


def probe_frontend(port: int, timeout: float = 1.5) -> bool:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=timeout) as resp:
            if resp.status != 200:
                return False
            body = resp.read(16384).decode("utf-8", "replace")
        return ("FrpPanel" in body) or ("/@vite/client" in body) or ("/src/main.ts" in body)
    except Exception:
        return False


def read_int_file(path: Path) -> int | None:
    try:
        value = int(path.read_text(encoding="utf-8").strip())
        if 1 <= value <= 65535:
            return value
    except Exception:
        pass
    return None


def discover_backend(preferred: int) -> tuple[int, dict] | None:
    """发现正在运行的 FrpPanel：优先 dev.bat 记录的端口，再试常见端口。"""
    candidates: list[int] = []
    saved = read_int_file(ROOT / "run" / "dev" / "backend.port")
    if saved:
        candidates.append(saved)
    candidates.extend([preferred, 8899, 8787, 8788, 8790, 18080, 18787])
    for port in dict.fromkeys(candidates):
        info = probe_backend(port)
        if info:
            return port, info
    return None


def discover_frontend(preferred: int) -> int | None:
    candidates: list[int] = []
    saved = read_int_file(ROOT / "run" / "dev" / "frontend.port")
    if saved:
        candidates.append(saved)
    candidates.extend([preferred, 5173, 5174, 5175, 4173])
    for port in dict.fromkeys(candidates):
        if probe_frontend(port):
            return port
    return None


def load_settings() -> dict:
    defaults = {
        "backend_port": BACKEND_PORT_DEFAULT,
        "frontend_port": FRONTEND_PORT_DEFAULT,
        "username": "admin",
        "auto_frontend": True,
        "auto_open": True,
    }
    try:
        data = json.loads(SETTINGS_FILE.read_text(encoding="utf-8"))
        for key in ("backend_port", "frontend_port"):
            value = int(data.get(key, defaults[key]))
            if 1 <= value <= 65535:
                defaults[key] = value
        if isinstance(data.get("username"), str) and data["username"].strip():
            defaults["username"] = data["username"].strip()
        defaults["auto_frontend"] = bool(data.get("auto_frontend", True))
        defaults["auto_open"] = bool(data.get("auto_open", True))
    except Exception:
        pass
    return defaults


def save_settings(data: dict) -> None:
    try:
        SETTINGS_FILE.parent.mkdir(parents=True, exist_ok=True)
        SETTINGS_FILE.write_text(
            json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8"
        )
    except Exception:
        pass


def popen_hidden(cmd: list[str], cwd: Path, env: dict[str, str]) -> subprocess.Popen:
    kwargs: dict = {
        "cwd": str(cwd),
        "env": env,
        "stdin": subprocess.DEVNULL,
        "stdout": subprocess.PIPE,
        "stderr": subprocess.STDOUT,
        "text": True,
        "encoding": "utf-8",
        "errors": "replace",
        "bufsize": 1,
    }
    if os.name == "nt":
        kwargs["creationflags"] = CREATE_NO_WINDOW
    else:
        kwargs["start_new_session"] = True
    return subprocess.Popen(cmd, **kwargs)


def terminate_tree(proc: subprocess.Popen | None) -> None:
    if proc is None or proc.poll() is not None:
        return
    try:
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                creationflags=CREATE_NO_WINDOW,
                timeout=12,
            )
        else:
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
    except Exception:
        try:
            proc.terminate()
        except Exception:
            pass
    try:
        proc.wait(timeout=6)
    except Exception:
        try:
            proc.kill()
        except Exception:
            pass


def panel_binary() -> Path:
    name = "frp-panel.exe" if os.name == "nt" else "frp-panel"
    return ROOT / "target" / "debug" / name


def _binary_is_current(exe: Path) -> bool:
    """Avoid running an old binary that predates the reset flag / crypto changes."""
    try:
        exe_time = exe.stat().st_mtime
        sources = [
            ROOT / "backend" / "src" / "main.rs",
            ROOT / "backend" / "src" / "crypto.rs",
            ROOT / "backend" / "src" / "db" / "repo.rs",
            ROOT / "Cargo.toml",
        ]
        return all(
            (not src.exists()) or src.stat().st_mtime <= exe_time + 1
            for src in sources
        )
    except Exception:
        return False


def reset_admin_password_local(
    username: str, new_password: str, data_dir: Path | None = None, timeout: int = 600
) -> str:
    """直接重置本地管理员密码，不经过登录，也不需要旧密码。

    通过 `frp-panel --reset-admin-password` 从 stdin 读取新口令，使用后端同一套
    Argon2 哈希写入 SQLite；后端正在运行也可以直接生效。
    """
    args = ["--reset-admin-password", "--admin-username", username]
    exe = panel_binary()
    if exe.exists() and _binary_is_current(exe):
        cmd = [str(exe), *args]
    else:
        cmd = ["cargo", "run", "-p", "frp-panel", "--", *args]

    env = os.environ.copy()
    env["PYTHONIOENCODING"] = "utf-8"
    if data_dir is not None and not (ROOT / "panel.toml").exists():
        try:
            env["FRP_PANEL_DATA_DIR"] = str(data_dir.resolve().relative_to(ROOT))
        except ValueError:
            env["FRP_PANEL_DATA_DIR"] = str(data_dir.resolve())

    kwargs: dict = {
        "cwd": str(ROOT),
        "env": env,
        "input": new_password + "\n",
        "text": True,
        "encoding": "utf-8",
        "errors": "replace",
        "capture_output": True,
        "timeout": timeout,
    }
    if os.name == "nt":
        kwargs["creationflags"] = CREATE_NO_WINDOW
    proc = subprocess.run(cmd, **kwargs)
    output = ((proc.stdout or "") + (proc.stderr or "")).strip()
    if proc.returncode != 0:
        raise RuntimeError(output[-1200:] or f"重置命令退出码 {proc.returncode}")
    if "OK" not in (proc.stdout or ""):
        raise RuntimeError("重置命令没有返回 OK：\n" + (output[-1200:] or "(无输出)"))
    return output


def detect_active_data_dir() -> Path:
    """在 data / data/dev 中挑选最近被写入的数据库目录。"""
    best = ROOT / "data"
    best_time = 0.0
    for directory in (ROOT / "data", ROOT / "data" / "dev"):
        for name in ("panel.db-wal", "panel.db"):
            path = directory / name
            if path.exists():
                try:
                    stamp = path.stat().st_mtime
                except OSError:
                    continue
                if stamp > best_time:
                    best, best_time = directory, stamp
    return best


def verify_admin_login(base_url: str, username: str, password: str) -> bool:
    try:
        pair = http_json(
            base_url.rstrip("/") + "/api/auth/login",
            method="POST",
            payload={"username": username, "password": password},
            timeout=8.0,
        )
        return bool(pair.get("access_token"))
    except Exception:
        return False


# ---------------------------------------------------------------------------
# 主窗口
# ---------------------------------------------------------------------------
class PanelGUI:
    def __init__(self) -> None:
        self.root = tk.Tk()
        self.root.title(APP_NAME)
        self.root.configure(bg=BG)
        self._center_window(1040, 800)
        self.root.minsize(920, 700)
        self._set_dpi()

        self.settings = load_settings()
        self._closing = False
        self._busy = False
        self._stop_requested = False
        self._backend_proc: subprocess.Popen | None = None
        self._frontend_proc: subprocess.Popen | None = None
        self._backend_attached = False
        self._active_backend_port = self.settings["backend_port"]
        self._active_frontend_port = self.settings["frontend_port"]
        self._last_backend: dict | None = None
        self._last_frontend_online = False
        self._events: queue.Queue = queue.Queue()
        self._logo_img = self._load_logo()
        self._apply_window_icon()

        self._build_ui()
        self._bind_shortcuts()
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)

        self._log("system", f"{APP_NAME} v{APP_VERSION}")
        self._log("system", f"项目目录：{ROOT}")
        self._log(
            "system",
            "提示：默认只监听 127.0.0.1；端口冲突时会自动换端口，具体见日志。",
        )

        self.root.after(80, self._drain_events)
        self.root.after(150, self._start_initial_discovery)
        threading.Thread(target=self._poll_loop, daemon=True).start()

    # ------------------------------------------------------------------
    # 窗口与主题
    # ------------------------------------------------------------------
    def _set_dpi(self) -> None:
        if os.name == "nt":
            try:
                ctypes.windll.shcore.SetProcessDpiAwareness(1)
            except Exception:
                pass

    def _center_window(self, width: int, height: int) -> None:
        sw = self.root.winfo_screenwidth()
        sh = self.root.winfo_screenheight()
        x = max(0, (sw - width) // 2)
        y = max(0, (sh - height) // 2 - 20)
        self.root.geometry(f"{width}x{height}+{x}+{y}")

    def _load_logo(self) -> tk.PhotoImage | None:
        path = ROOT / "frontend" / "src" / "assets" / "logo" / "frppanel-mark.png"
        try:
            img = tk.PhotoImage(file=str(path))
            factor = max(1, round(img.width() / 42))
            scaled = img.subsample(factor, factor)
            return scaled
        except Exception:
            return None

    def _apply_window_icon(self) -> None:
        # 优先使用新生成的 ICO，没有就回退到 PNG
        ico = ROOT / "run" / "panel_gui.ico"
        try:
            if ico.exists():
                self.root.iconbitmap(str(ico))
                return
        except Exception:
            pass
        png = ROOT / "frontend" / "src" / "assets" / "logo" / "frppanel-mark.png"
        try:
            if png.exists():
                self.root.iconphoto(True, tk.PhotoImage(file=str(png)))
        except Exception:
            pass

    def _card(self, parent: tk.Widget, title: str, subtitle: str = "") -> tuple[tk.Frame, tk.Frame, tk.Frame]:
        outer = tk.Frame(
            parent,
            bg=CARD,
            highlightbackground=BORDER,
            highlightthickness=1,
            bd=0,
        )
        inner = tk.Frame(outer, bg=CARD)
        inner.pack(fill="both", expand=True, padx=18, pady=14)
        head = tk.Frame(inner, bg=CARD)
        head.pack(fill="x")
        tk.Label(
            head, text=title, bg=CARD, fg=TEXT, font=(FONT_UI, 11, "bold")
        ).pack(side="left")
        if subtitle:
            tk.Label(
                head, text=subtitle, bg=CARD, fg=FAINT, font=(FONT_UI, 9)
            ).pack(side="left", padx=(10, 0), pady=(2, 0))
        body = tk.Frame(inner, bg=CARD)
        body.pack(fill="both", expand=True, pady=(10, 0))
        return outer, body, head

    def _label(self, parent: tk.Widget, text: str, muted: bool = False) -> tk.Label:
        return tk.Label(
            parent,
            text=text,
            bg=CARD,
            fg=MUTED if muted else TEXT,
            font=(FONT_UI, 10),
        )

    def _entry(
        self,
        parent: tk.Widget,
        variable: tk.StringVar,
        width: int,
        *,
        show: str | None = None,
        font: tuple = (FONT_MONO, 10),
    ) -> tk.Entry:
        entry = tk.Entry(
            parent,
            textvariable=variable,
            width=width,
            font=font,
            bg="#FBFCFD",
            fg=TEXT,
            disabledbackground="#F1F3F5",
            disabledforeground=FAINT,
            insertbackground=TEXT,
            relief="flat",
            bd=4,
            highlightthickness=1,
            highlightbackground=BORDER,
            highlightcolor=ACCENT,
            show=show or "",
        )
        return entry

    def _button(
        self,
        parent: tk.Widget,
        text: str,
        command,
        kind: str = "secondary",
        width: int | None = None,
    ) -> tk.Button:
        palette = {
            "primary": (ACCENT, "#FFFFFF", ACCENT_DARK),
            "secondary": ("#F1F4F8", TEXT, "#E4E9F0"),
            "danger": ("#FEF2F2", DANGER, "#FEE2E2"),
            "ghost": (CARD, MUTED, "#F1F3F5"),
        }
        bg, fg, active = palette.get(kind, palette["secondary"])
        btn = tk.Button(
            parent,
            text=text,
            command=command,
            bg=bg,
            fg=fg,
            activebackground=active,
            activeforeground=fg,
            disabledforeground="#A8AEB8",
            font=(FONT_UI, 10),
            relief="flat",
            bd=0,
            padx=14,
            pady=6,
            cursor="hand2",
            highlightthickness=0,
        )
        if width:
            btn.configure(width=width)
        return btn

    def _pill(self, parent: tk.Widget, text: str) -> tk.Label:
        return tk.Label(
            parent,
            text=text,
            bg=FAINT,
            fg="#FFFFFF",
            font=(FONT_UI, 9, "bold"),
            padx=10,
            pady=4,
        )

    # ------------------------------------------------------------------
    # UI 构建
    # ------------------------------------------------------------------
    def _build_ui(self) -> None:
        outer = tk.Frame(self.root, bg=BG)
        outer.pack(fill="both", expand=True, padx=20, pady=16)

        # ---- 顶栏 ----
        header = tk.Frame(outer, bg=BG)
        header.pack(fill="x")
        if self._logo_img is not None:
            tk.Label(header, image=self._logo_img, bg=BG).pack(side="left", padx=(0, 12))
        title_box = tk.Frame(header, bg=BG)
        title_box.pack(side="left")
        tk.Label(
            title_box, text="FrpPanel", bg=BG, fg=TEXT, font=(FONT_UI, 18, "bold")
        ).pack(anchor="w")
        tk.Label(
            title_box,
            text="启动 / 停止 · 实时日志 · 管理员登录密码",
            bg=BG,
            fg=MUTED,
            font=(FONT_UI, 9),
        ).pack(anchor="w")

        pills = tk.Frame(header, bg=BG)
        pills.pack(side="right", pady=(6, 0))
        self.backend_pill = self._pill(pills, "后端 · 检测中")
        self.backend_pill.pack(side="left", padx=(0, 8))
        self.frontend_pill = self._pill(pills, "前端 · 检测中")
        self.frontend_pill.pack(side="left")

        # ---- 服务控制 ----
        control_outer, control, _ = self._card(
            outer, "服务控制", "后端 cargo run · 前端 Vite（开发热更新）"
        )
        control_outer.pack(fill="x", pady=(16, 0))

        row = tk.Frame(control, bg=CARD)
        row.pack(fill="x")
        self._label(row, "后端端口").pack(side="left")
        self.backend_port_var = tk.StringVar(value=str(self._active_backend_port))
        self.backend_port_entry = self._entry(row, self.backend_port_var, 8)
        self.backend_port_entry.pack(side="left", padx=(8, 18))

        self._label(row, "前端端口").pack(side="left")
        self.frontend_port_var = tk.StringVar(value=str(self._active_frontend_port))
        self.frontend_port_entry = self._entry(row, self.frontend_port_var, 8)
        self.frontend_port_entry.pack(side="left", padx=(8, 20))

        self.auto_frontend_var = tk.BooleanVar(value=self.settings["auto_frontend"])
        self.auto_frontend_check = tk.Checkbutton(
            row,
            text="同时启动前端",
            variable=self.auto_frontend_var,
            bg=CARD,
            fg=TEXT,
            activebackground=CARD,
            activeforeground=TEXT,
            selectcolor=CARD,
            font=(FONT_UI, 10),
            cursor="hand2",
            highlightthickness=0,
        )
        self.auto_frontend_check.pack(side="left", padx=(0, 14))

        self.auto_open_var = tk.BooleanVar(value=self.settings["auto_open"])
        self.auto_open_check = tk.Checkbutton(
            row,
            text="启动后打开面板",
            variable=self.auto_open_var,
            bg=CARD,
            fg=TEXT,
            activebackground=CARD,
            activeforeground=TEXT,
            selectcolor=CARD,
            font=(FONT_UI, 10),
            cursor="hand2",
            highlightthickness=0,
        )
        self.auto_open_check.pack(side="left")

        # 地址行
        url_row = tk.Frame(control, bg=CARD)
        url_row.pack(fill="x", pady=(10, 0))
        self.backend_url_label = tk.Label(
            url_row,
            text=f"后端  http://127.0.0.1:{self._active_backend_port}",
            bg=CARD,
            fg=MUTED,
            font=(FONT_MONO, 9),
            cursor="hand2",
        )
        self.backend_url_label.pack(side="left")
        tk.Label(url_row, text="·", bg=CARD, fg=FAINT).pack(side="left", padx=8)
        self.frontend_url_label = tk.Label(
            url_row,
            text=f"前端  http://127.0.0.1:{self._active_frontend_port}",
            bg=CARD,
            fg=MUTED,
            font=(FONT_MONO, 9),
            cursor="hand2",
        )
        self.frontend_url_label.pack(side="left")
        self.backend_url_label.bind("<Button-1>", lambda _e: self._open_backend())
        self.frontend_url_label.bind("<Button-1>", lambda _e: self._open_panel())

        # 按钮行
        buttons = tk.Frame(control, bg=CARD)
        buttons.pack(fill="x", pady=(12, 0))
        self.start_btn = self._button(buttons, "启动服务", self.start_all, "primary")
        self.start_btn.pack(side="left")
        self.stop_btn = self._button(buttons, "停止服务", self.stop_all, "danger")
        self.stop_btn.pack(side="left", padx=(8, 0))
        self.restart_btn = self._button(buttons, "重启", self.restart, "secondary")
        self.restart_btn.pack(side="left", padx=(8, 0))
        self.open_btn = self._button(buttons, "打开面板", self._open_panel, "secondary")
        self.open_btn.pack(side="left", padx=(8, 0))
        self.refresh_btn = self._button(buttons, "刷新状态", self._refresh_now, "ghost")
        self.refresh_btn.pack(side="left", padx=(8, 0))
        self._button(buttons, "打开数据目录", self._open_data_dir, "ghost").pack(
            side="right"
        )

        self.busy_label = tk.Label(
            control, text="", bg=CARD, fg=WARNING, font=(FONT_UI, 9)
        )
        self.busy_label.pack(anchor="w", pady=(8, 0))

        # ---- 登录密码 ----
        password_outer, password, _ = self._card(
            outer,
            "管理员登录密码",
            "忘记旧密码也能重置；新口令直接写入本地面板数据库",
        )
        password_outer.pack(fill="x", pady=(12, 0))

        prow = tk.Frame(password, bg=CARD)
        prow.pack(fill="x")
        self._label(prow, "用户名").pack(side="left")
        self.username_var = tk.StringVar(value=self.settings["username"])
        self._entry(prow, self.username_var, 8, font=(FONT_UI, 10)).pack(
            side="left", padx=(8, 14)
        )

        self._label(prow, "新密码").pack(side="left")
        self.new_pw_var = tk.StringVar()
        self.new_pw_entry = self._entry(prow, self.new_pw_var, 16, show="•")
        self.new_pw_entry.pack(side="left", padx=(8, 14))

        self._label(prow, "确认新密码").pack(side="left")
        self.confirm_pw_var = tk.StringVar()
        self.confirm_pw_entry = self._entry(prow, self.confirm_pw_var, 16, show="•")
        self.confirm_pw_entry.pack(side="left", padx=(8, 10))

        self.show_pw_var = tk.BooleanVar(value=False)
        tk.Checkbutton(
            prow,
            text="显示",
            variable=self.show_pw_var,
            command=self._toggle_password_visibility,
            bg=CARD,
            fg=MUTED,
            activebackground=CARD,
            activeforeground=TEXT,
            selectcolor=CARD,
            font=(FONT_UI, 9),
            cursor="hand2",
            highlightthickness=0,
        ).pack(side="left")

        prow2 = tk.Frame(password, bg=CARD)
        prow2.pack(fill="x", pady=(10, 0))
        self.change_pw_btn = self._button(
            prow2, "直接重置密码", self.reset_password, "primary"
        )
        self.change_pw_btn.pack(side="left")
        tk.Label(
            prow2,
            text="新密码至少 8 位；无需旧密码，重置后旧密码立即失效。",
            bg=CARD,
            fg=FAINT,
            font=(FONT_UI, 9),
        ).pack(side="left", padx=(12, 0))
        self._button(prow2, "查看初始口令", self._show_initial_password, "ghost").pack(
            side="right"
        )

        self.pw_target_label = tk.Label(
            password,
            text=f"目标数据库：{detect_active_data_dir() / 'panel.db'}",
            bg=CARD,
            fg=FAINT,
            font=(FONT_MONO, 8),
        )
        self.pw_target_label.pack(anchor="w", pady=(7, 0))

        # ---- 日志 ----
        log_outer, log_body, log_head = self._card(
            outer, "运行日志", "后端 / 前端输出会实时显示在这里"
        )
        log_outer.pack(fill="both", expand=True, pady=(12, 0))

        self.auto_scroll_var = tk.BooleanVar(value=True)
        tk.Checkbutton(
            log_head,
            text="自动滚动",
            variable=self.auto_scroll_var,
            bg=CARD,
            fg=MUTED,
            activebackground=CARD,
            activeforeground=TEXT,
            selectcolor=CARD,
            font=(FONT_UI, 9),
            cursor="hand2",
            highlightthickness=0,
        ).pack(side="right", padx=(8, 0))
        self._button(log_head, "复制", self._copy_log, "ghost").pack(side="right")
        self._button(log_head, "清空", self._clear_log, "ghost").pack(
            side="right", padx=(0, 6)
        )

        text_frame = tk.Frame(log_body, bg=LOG_BG)
        text_frame.pack(fill="both", expand=True)
        self.log_text = tk.Text(
            text_frame,
            bg=LOG_BG,
            fg=LOG_FG,
            insertbackground=LOG_FG,
            font=(FONT_MONO, 9),
            relief="flat",
            bd=0,
            padx=10,
            pady=8,
            height=11,
            wrap="word",
            state="disabled",
            selectbackground="#2A3444",
        )
        scrollbar = ttk.Scrollbar(text_frame, orient="vertical", command=self.log_text.yview)
        scrollbar.pack(side="right", fill="y")
        self.log_text.configure(yscrollcommand=scrollbar.set)
        self.log_text.pack(side="left", fill="both", expand=True)
        self.log_text.tag_configure("system", foreground="#93A1B5")
        self.log_text.tag_configure("backend", foreground="#7DD3FC")
        self.log_text.tag_configure("frontend", foreground="#A5B4FC")
        self.log_text.tag_configure("success", foreground="#4ADE80")
        self.log_text.tag_configure("warning", foreground="#FBBF24")
        self.log_text.tag_configure("error", foreground="#F87171")
        self.log_text.tag_configure("plain", foreground=LOG_FG)

        footer = tk.Frame(outer, bg=BG)
        footer.pack(fill="x", pady=(8, 0))
        self.footer_data_label = tk.Label(
            footer,
            text=f"数据目录：{ROOT / 'data'}",
            bg=BG,
            fg=FAINT,
            font=(FONT_UI, 8),
        )
        self.footer_data_label.pack(side="left")
        tk.Label(
            footer,
            text="日志最多保留 3000 行 · 不保存密码",
            bg=BG,
            fg=FAINT,
            font=(FONT_UI, 8),
        ).pack(side="right")

        self._update_controls()

    def _bind_shortcuts(self) -> None:
        self.root.bind("<F5>", lambda _e: self.start_all())
        self.root.bind("<Control-l>", lambda _e: self._clear_log())
        self.confirm_pw_entry.bind("<Return>", lambda _e: self.reset_password())
        self.new_pw_entry.bind("<Return>", lambda _e: self.reset_password())

    # ------------------------------------------------------------------
    # 状态与事件
    # ------------------------------------------------------------------
    def _log(self, level: str, text: str) -> None:
        self._events.put(("log", level, text))

    def _append_log(self, level: str, text: str) -> None:
        stamp = time.strftime("%H:%M:%S")
        self.log_text.configure(state="normal")
        self.log_text.insert("end", f"[{stamp}] ", "system")
        self.log_text.insert("end", text.rstrip() + "\n", level if level else "plain")
        # 控制体积
        try:
            lines = int(self.log_text.index("end-1c").split(".")[0])
            if lines > 3000:
                self.log_text.delete("1.0", f"{lines - 3000}.0")
        except Exception:
            pass
        if self.auto_scroll_var.get():
            self.log_text.see("end")
        self.log_text.configure(state="disabled")

    def _clear_log(self) -> None:
        self.log_text.configure(state="normal")
        self.log_text.delete("1.0", "end")
        self.log_text.configure(state="disabled")

    def _copy_log(self) -> None:
        content = self.log_text.get("1.0", "end-1c")
        self.root.clipboard_clear()
        self.root.clipboard_append(content)
        self._append_log("system", "日志已复制到剪贴板。")

    def _drain_events(self) -> None:
        try:
            for _ in range(240):
                event = self._events.get_nowait()
                kind = event[0]
                if kind == "log":
                    self._append_log(event[1], str(event[2]))
                elif kind == "status":
                    self._last_backend = event[1]
                    self._last_frontend_online = bool(event[2])
                    self._render_status()
                elif kind == "ports":
                    _, bp, up = event
                    self._active_backend_port = bp
                    self._active_frontend_port = up
                    self.backend_port_var.set(str(bp))
                    self.frontend_port_var.set(str(up))
                    self._update_urls()
                elif kind == "discovered":
                    _, bp, info = event
                    if self._busy or self._backend_proc is not None:
                        continue
                    self._backend_attached = True
                    self._active_backend_port = bp
                    self.backend_port_var.set(str(bp))
                    self._update_urls()
                    self._append_log(
                        "success",
                        f"发现已在运行的后端：http://127.0.0.1:{bp}（v{info.get('version', '?')}），已接管显示。",
                    )
                elif kind == "ui_discovered":
                    up = event[1]
                    if self._busy or self._frontend_proc is not None:
                        continue
                    self._active_frontend_port = up
                    self.frontend_port_var.set(str(up))
                    self._update_urls()
                    self._append_log("success", f"发现已在运行的前端：http://127.0.0.1:{up}")
                elif kind == "busy":
                    self._set_busy(bool(event[1]), str(event[2]) if len(event) > 2 else "")
                elif kind == "busy_msg":
                    self.busy_label.configure(text=str(event[1]))
                elif kind == "proc_exit":
                    _, which, code = event
                    self._append_log(
                        "error",
                        f"{'后端' if which == 'backend' else '前端'}进程已退出（code {code}）。",
                    )
                    self._update_controls()
                elif kind == "started":
                    _, bp, up, auto_open = event
                    self._save_settings()
                    self._update_urls()
                    if auto_open:
                        self._open_panel()
                elif kind == "password":
                    _, ok, message = event
                    self._set_busy(False, "")
                    self.change_pw_btn.configure(state="normal")
                    if ok:
                        self.new_pw_var.set("")
                        self.confirm_pw_var.set("")
                        self._append_log("success", message)
                        messagebox.showinfo("重置成功", message, parent=self.root)
                    else:
                        self._append_log("error", message)
                        messagebox.showerror("重置失败", message, parent=self.root)
                elif kind == "error_dialog":
                    _, title, message = event
                    messagebox.showerror(title, message, parent=self.root)
                elif kind == "open":
                    webbrowser.open(str(event[1]))
        except queue.Empty:
            pass
        finally:
            if not self._closing:
                self.root.after(100, self._drain_events)

    def _render_status(self) -> None:
        b = self._last_backend
        f = self._last_frontend_online

        if b:
            suffix = "（外部）" if self._backend_attached else ""
            self.backend_pill.configure(text=f"后端 · 运行中{suffix}", bg=SUCCESS)
        elif self._busy and (self._backend_proc is not None or self._backend_attached):
            self.backend_pill.configure(text="后端 · 启动中", bg=WARNING)
        else:
            self.backend_pill.configure(text="后端 · 已停止", bg=FAINT)

        if f:
            self.frontend_pill.configure(text="前端 · 运行中", bg=SUCCESS)
        elif self._busy and self._frontend_proc is not None:
            self.frontend_pill.configure(text="前端 · 启动中", bg=WARNING)
        else:
            self.frontend_pill.configure(text="前端 · 已停止", bg=FAINT)

        self._update_urls()
        self._update_controls()

    def _update_urls(self) -> None:
        self.backend_url_label.configure(
            text=f"后端  http://127.0.0.1:{self._active_backend_port}"
        )
        self.frontend_url_label.configure(
            text=f"前端  http://127.0.0.1:{self._active_frontend_port}"
        )
        target = self._target_data_dir()
        self.pw_target_label.configure(text=f"目标数据库：{target / 'panel.db'}")
        self.footer_data_label.configure(text=f"数据目录：{target}")

    def _set_busy(self, busy: bool, message: str = "") -> None:
        self._busy = busy
        self.busy_label.configure(text=message if busy else "")
        state = "disabled" if busy else "normal"
        for widget in (
            self.start_btn,
            self.stop_btn,
            self.restart_btn,
            self.refresh_btn,
            self.backend_port_entry,
            self.frontend_port_entry,
            self.auto_frontend_check,
            self.auto_open_check,
        ):
            try:
                widget.configure(state=state)
            except Exception:
                pass
        self._update_controls()

    def _update_controls(self) -> None:
        if self._busy:
            return
        running = bool(self._last_backend) or bool(self._last_frontend_online)
        own_proc = self._backend_proc is not None or self._frontend_proc is not None
        self.start_btn.configure(state="normal")
        self.stop_btn.configure(state="normal" if own_proc else "disabled")
        self.restart_btn.configure(state="normal" if running else "disabled")
        self.open_btn.configure(state="normal" if running else "disabled")
        self.backend_port_entry.configure(
            state="disabled" if self._last_backend else "normal"
        )
        self.frontend_port_entry.configure(
            state="disabled" if self._last_frontend_online else "normal"
        )

    # ------------------------------------------------------------------
    # 轮询
    # ------------------------------------------------------------------
    def _poll_loop(self) -> None:
        while not self._closing:
            try:
                bp = self._active_backend_port
                up = self._active_frontend_port
                backend = probe_backend(bp)
                frontend = probe_frontend(up)

                for name, proc in (
                    ("backend", self._backend_proc),
                    ("frontend", self._frontend_proc),
                ):
                    if proc is not None and proc.poll() is not None and not self._stop_requested:
                        self._events.put(("proc_exit", name, proc.returncode))
                        if name == "backend":
                            self._backend_proc = None
                        else:
                            self._frontend_proc = None

                self._events.put(("status", backend, frontend))
            except Exception:
                pass
            time.sleep(1.8)

    def _start_initial_discovery(self) -> None:
        def work() -> None:
            found = discover_backend(self._active_backend_port)
            if found:
                port, info = found
                self._events.put(("discovered", port, info))
            ui = discover_frontend(self._active_frontend_port)
            if ui:
                self._events.put(("ui_discovered", ui))

        threading.Thread(target=work, daemon=True).start()

    def _refresh_now(self) -> None:
        self._append_log("system", "刷新状态…")

        def work() -> None:
            bp = self._active_backend_port
            up = self._active_frontend_port
            self._events.put(("status", probe_backend(bp), probe_frontend(up)))

        threading.Thread(target=work, daemon=True).start()

    # ------------------------------------------------------------------
    # 启动 / 停止
    # ------------------------------------------------------------------
    def _read_ports(self) -> tuple[int, int] | None:
        try:
            bp = int(self.backend_port_var.get().strip())
            up = int(self.frontend_port_var.get().strip())
        except ValueError:
            messagebox.showerror("端口错误", "端口必须是 1-65535 之间的数字。", parent=self.root)
            return None
        if not (1 <= bp <= 65535) or not (1 <= up <= 65535):
            messagebox.showerror("端口错误", "端口必须在 1-65535 之间。", parent=self.root)
            return None
        if bp == up:
            messagebox.showerror("端口错误", "后端端口和前端端口不能相同。", parent=self.root)
            return None
        return bp, up

    def start_all(self) -> None:
        if self._busy:
            return
        ports = self._read_ports()
        if ports is None:
            return
        bp, up = ports
        auto_ui = self.auto_frontend_var.get()
        auto_open = self.auto_open_var.get()
        self._set_busy(True, "正在启动服务…")
        self._append_log("system", "开始启动服务。")
        threading.Thread(
            target=self._start_worker,
            args=(bp, up, auto_ui, auto_open, False),
            daemon=True,
        ).start()

    def restart(self) -> None:
        if self._busy:
            return
        if self._backend_attached and not self._backend_proc:
            messagebox.showwarning(
                "无法重启",
                "当前后端由外部进程管理（例如 dev.bat），本工具不能停止它。\n\n"
                "请先运行 dev.bat stop，或在任务管理器结束 frp-panel 后再试。",
                parent=self.root,
            )
            return
        ports = self._read_ports()
        if ports is None:
            return
        bp, up = ports
        auto_ui = self.auto_frontend_var.get()
        auto_open = self.auto_open_var.get()
        self._set_busy(True, "正在重启服务…")
        threading.Thread(
            target=self._start_worker,
            args=(bp, up, auto_ui, auto_open, True),
            daemon=True,
        ).start()

    def stop_all(self) -> None:
        if self._busy:
            return
        if not (self._backend_proc or self._frontend_proc):
            messagebox.showinfo(
                "无需停止",
                "当前没有由本工具启动的进程。\n\n"
                "如果面板仍在运行，可能是 dev.bat 或其他方式启动的；请使用 dev.bat stop 停止。",
                parent=self.root,
            )
            return
        if not messagebox.askyesno("停止服务", "确定停止由本工具启动的后端 / 前端进程吗？", parent=self.root):
            return
        self._set_busy(True, "正在停止服务…")
        threading.Thread(target=self._stop_worker, daemon=True).start()

    def _stop_worker(self) -> None:
        try:
            self._stop_blocking()
            self._events.put(("log", "success", "服务已停止。"))
        except Exception as exc:
            self._events.put(("log", "error", f"停止失败：{exc}"))
        finally:
            self._events.put(("busy", False, ""))
            self._events.put(("status", probe_backend(self._active_backend_port), probe_frontend(self._active_frontend_port)))

    def _stop_blocking(self) -> None:
        self._stop_requested = True
        try:
            if self._frontend_proc is not None:
                self._events.put(("log", "system", "正在停止前端（Vite）…"))
                terminate_tree(self._frontend_proc)
                self._frontend_proc = None
            if self._backend_proc is not None:
                self._events.put(("log", "system", "正在停止后端（cargo / frp-panel）…"))
                terminate_tree(self._backend_proc)
                self._backend_proc = None
            if self._backend_attached and self._last_backend:
                self._events.put(
                    ("log", "warning", "后端由外部进程管理，本工具不会停止它。"),
                )
            time.sleep(0.4)
        finally:
            self._stop_requested = False

    def _start_worker(
        self, bp: int, up: int, auto_ui: bool, auto_open: bool, restarting: bool
    ) -> None:
        try:
            if restarting:
                self._events.put(("log", "system", "重启：先停止本工具启动的进程…"))
                self._stop_blocking()
                self._backend_attached = False
                time.sleep(0.3)

            # ---- 后端 ----
            info = probe_backend(bp)
            if not info:
                # dev.bat 或上一次运行可能把面板启动在别的端口；先找出来复用，
                # 避免同一个项目同时跑两个后端。
                found = discover_backend(bp)
                if found:
                    found_port, found_info = found
                    if found_port != bp:
                        self._events.put(("ports", found_port, up))
                        self._events.put(
                            (
                                "log",
                                "warning",
                                f"后端不在端口 {bp}，但在 {found_port} 发现已运行实例，改为复用。",
                            )
                        )
                        bp = found_port
                    info = found_info
            if info:
                self._backend_attached = True
                self._backend_proc = None
                self._events.put(
                    (
                        "log",
                        "success",
                        f"后端已在运行：http://127.0.0.1:{bp}（v{info['version']}），直接复用。",
                    )
                )
            else:
                if port_in_use(bp):
                    new_bp, changed = pick_port(bp, BACKEND_PORT_ALTS)
                    if changed:
                        self._events.put(
                            (
                                "log",
                                "warning",
                                f"后端端口 {bp} 被其他程序占用，自动改用 {new_bp}。",
                            )
                        )
                        self._events.put(("ports", new_bp, up))
                        bp = new_bp
                self._check_command("cargo", "未找到 cargo，请先安装 Rust 工具链（https://rustup.rs）。")
                self._events.put(("busy_msg", "正在编译 / 启动后端（首次可能需要 1-3 分钟）…"))
                self._events.put(
                    (
                        "log",
                        "backend",
                        f"$ cargo run -p frp-panel -- --bind 127.0.0.1:{bp}",
                    )
                )
                cmd = [
                    "cargo",
                    "run",
                    "-p",
                    "frp-panel",
                    "--",
                    "--bind",
                    f"127.0.0.1:{bp}",
                ]
                proc = popen_hidden(cmd, ROOT, self._backend_env())
                self._backend_proc = proc
                self._backend_attached = False
                self._events.put(("log", "system", f"后端进程 PID {proc.pid}"))
                self._start_reader(proc, "backend")
                info = self._wait_backend(bp)
                self._events.put(
                    (
                        "log",
                        "success",
                        f"后端就绪：v{info['version']} · http://127.0.0.1:{bp}",
                    )
                )
                self._announce_initial_password()

            self._active_backend_port = bp
            self._events.put(("status", info, self._last_frontend_online))

            # ---- 前端 ----
            if auto_ui:
                existing_up = discover_frontend(up)
                if existing_up:
                    if existing_up != up:
                        up = existing_up
                        self._events.put(("ports", bp, up))
                    self._events.put(
                        ("log", "success", f"前端已在运行：http://127.0.0.1:{up}，直接复用。")
                    )
                else:
                    if port_in_use(up):
                        new_up, changed = pick_port(up, FRONTEND_PORT_ALTS)
                        if changed:
                            self._events.put(
                                (
                                    "log",
                                    "warning",
                                    f"前端端口 {up} 被占用，自动改用 {new_up}。",
                                )
                            )
                            self._events.put(("ports", bp, new_up))
                            up = new_up
                    self._ensure_frontend_deps()
                    self._events.put(("busy_msg", "正在启动前端（Vite）…"))
                    env = os.environ.copy()
                    env["FRP_PANEL_API"] = f"http://127.0.0.1:{bp}"
                    env["BROWSER"] = "none"
                    env["PYTHONIOENCODING"] = "utf-8"
                    if os.name == "nt":
                        cmd = [
                            "cmd.exe",
                            "/d",
                            "/c",
                            f"npm run dev -- --host 127.0.0.1 --port {up} --strictPort",
                        ]
                    else:
                        cmd = [
                            "npm",
                            "run",
                            "dev",
                            "--",
                            "--host",
                            "127.0.0.1",
                            "--port",
                            str(up),
                            "--strictPort",
                        ]
                    self._events.put(
                        (
                            "log",
                            "frontend",
                            f"$ npm run dev -- --host 127.0.0.1 --port {up} --strictPort",
                        )
                    )
                    proc = popen_hidden(cmd, FRONTEND_DIR, env)
                    self._frontend_proc = proc
                    self._events.put(("log", "system", f"前端进程 PID {proc.pid}"))
                    self._start_reader(proc, "frontend")
                    self._wait_frontend(up)
                    self._events.put(
                        ("log", "success", f"前端就绪：http://127.0.0.1:{up}")
                    )

            self._active_frontend_port = up
            self._events.put(
                (
                    "started",
                    self._active_backend_port,
                    self._active_frontend_port,
                    auto_open,
                )
            )
            self._events.put(("status", probe_backend(bp), probe_frontend(up)))
        except Exception as exc:
            self._events.put(("log", "error", f"启动失败：{exc}"))
            self._events.put(("error_dialog", "启动失败", str(exc)))
        finally:
            self._events.put(("busy", False, ""))
            self._events.put(
                (
                    "status",
                    probe_backend(self._active_backend_port),
                    probe_frontend(self._active_frontend_port),
                )
            )

    def _backend_env(self) -> dict[str, str]:
        env = os.environ.copy()
        env.setdefault(
            "RUST_LOG", "info,frp_panel=debug,tower_http=warn,russh=warn"
        )
        # 开发模式由 Vite 提供前端；不要误挂载 dist
        env["FRP_PANEL_STATIC_DIR"] = ""
        env["PYTHONIOENCODING"] = "utf-8"
        return env

    def _check_command(self, name: str, message: str) -> None:
        if shutil.which(name) is None:
            raise RuntimeError(message)

    def _wait_backend(self, port: int, timeout: float = 240.0) -> dict:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self._stop_requested:
                raise RuntimeError("启动已被取消。")
            info = probe_backend(port)
            if info:
                return info
            proc = self._backend_proc
            if proc is not None and proc.poll() is not None:
                raise RuntimeError(
                    f"后端进程提前退出（code {proc.returncode}）。"
                    "请查看日志中的编译错误或端口占用信息。"
                )
            time.sleep(0.7)
        raise RuntimeError(
            f"等待后端就绪超时（{int(timeout)}s）。请查看日志，确认 cargo 是否编译失败。"
        )

    def _wait_frontend(self, port: int, timeout: float = 90.0) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self._stop_requested:
                raise RuntimeError("启动已被取消。")
            if probe_frontend(port):
                return
            proc = self._frontend_proc
            if proc is not None and proc.poll() is not None:
                raise RuntimeError(
                    f"前端进程提前退出（code {proc.returncode}）。请查看日志中的 npm 输出。"
                )
            time.sleep(0.6)
        raise RuntimeError(
            f"等待前端就绪超时（{int(timeout)}s）。请查看日志，确认 npm 依赖是否安装完成。"
        )

    def _ensure_frontend_deps(self) -> None:
        if (FRONTEND_DIR / "node_modules" / "vite" / "package.json").exists():
            return
        self._check_command("node", "未找到 Node.js，请安装 Node.js 20+（https://nodejs.org）。")
        self._events.put(
            ("log", "system", "未检测到前端依赖，正在执行 npm install（首次较慢）…")
        )
        self._events.put(("busy_msg", "正在安装前端依赖 npm install…"))
        env = os.environ.copy()
        env["PYTHONIOENCODING"] = "utf-8"
        if os.name == "nt":
            cmd = ["cmd.exe", "/d", "/c", "npm install --no-audit --no-fund"]
        else:
            cmd = ["npm", "install", "--no-audit", "--no-fund"]
        proc = popen_hidden(cmd, FRONTEND_DIR, env)
        self._start_reader(proc, "frontend")
        code = proc.wait()
        if code != 0:
            raise RuntimeError(f"npm install 失败（exit {code}），请查看日志。")

    def _start_reader(self, proc: subprocess.Popen, source: str) -> None:
        def read() -> None:
            try:
                if proc.stdout is None:
                    return
                for line in proc.stdout:
                    line = strip_ansi(line.rstrip("\r\n"))
                    if line:
                        self._events.put(("log", source, line))
            except Exception:
                pass

        threading.Thread(target=read, daemon=True).start()

    def _announce_initial_password(self) -> None:
        path = self._target_data_dir() / "admin.initial"
        if path.exists():
            self._events.put(
                (
                    "log",
                    "warning",
                    f"首次初始化：管理员口令已写入 {path}。"
                    "点击「查看初始口令」查看，或直接在上方重置密码。",
                )
            )

    # ------------------------------------------------------------------
    # 修改密码 / 其他动作
    # ------------------------------------------------------------------
    def _toggle_password_visibility(self) -> None:
        show = "" if self.show_pw_var.get() else "•"
        for entry in (self.new_pw_entry, self.confirm_pw_entry):
            entry.configure(show=show)

    def _target_data_dir(self) -> Path:
        env_dir = os.environ.get("FRP_PANEL_DATA_DIR", "").strip()
        if env_dir:
            path = Path(env_dir)
            return path if path.is_absolute() else ROOT / path
        # data / data/dev 中最近被写入的那个（通常是当前正在用的数据库）。
        return detect_active_data_dir()

    def reset_password(self) -> None:
        if self._busy:
            return
        username = self.username_var.get().strip()
        new = self.new_pw_var.get()
        confirm = self.confirm_pw_var.get()
        if not username:
            messagebox.showerror("输入不完整", "请输入管理员用户名。", parent=self.root)
            return
        if len(new) < 8:
            messagebox.showerror("密码太短", "新密码至少 8 位。", parent=self.root)
            return
        if new != confirm:
            messagebox.showerror("两次不一致", "新密码和确认密码不一致。", parent=self.root)
            return

        data_dir = self._target_data_dir()
        if not messagebox.askyesno(
            "直接重置密码",
            f"将直接把用户「{username}」的登录密码重置为新密码。\n\n"
            "不需要旧密码；旧密码会立即失效。\n"
            f"目标数据库：{data_dir / 'panel.db'}\n\n"
            "确定继续吗？",
            parent=self.root,
        ):
            return

        backend_online = self._last_backend is not None
        self._set_busy(True, "正在直接重置登录密码（首次可能需要编译后端）…")
        self.change_pw_btn.configure(state="disabled")
        self._append_log(
            "system",
            f"正在本地重置 {username} 的登录密码（目标：{data_dir / 'panel.db'}）…",
        )

        def work() -> None:
            try:
                output = reset_admin_password_local(username, new, data_dir=data_dir)
                last = output.splitlines()[-1] if output else "OK"
                self._events.put(("log", "system", f"重置命令完成：{last}"))
                if backend_online:
                    if verify_admin_login(
                        f"http://127.0.0.1:{self._active_backend_port}", username, new
                    ):
                        message = (
                            "管理员登录密码已重置，运行中的后端已用新密码验证通过。\n"
                            "请退出当前登录，再用新密码登录面板。"
                        )
                    else:
                        message = (
                            "密码已写入本地数据库，但运行中的面板仍无法用新密码登录。\n\n"
                            "它可能使用了另一个数据目录。请重启面板，或检查：\n"
                            f"{data_dir / 'panel.db'}"
                        )
                        self._events.put(("log", "warning", message))
                else:
                    message = (
                        "管理员登录密码已重置（后端未运行）。\n"
                        "下次启动面板后，用新密码登录即可。"
                    )
                self._events.put(("password", True, message))
            except subprocess.TimeoutExpired:
                self._events.put(
                    ("password", False, "重置超时：首次编译后端可能需要更久，请重试。")
                )
            except FileNotFoundError as exc:
                self._events.put(
                    (
                        "password",
                        False,
                        f"找不到重置工具：{exc}\n请先安装 Rust 工具链（cargo）。",
                    )
                )
            except Exception as exc:
                self._events.put(("password", False, f"重置失败：{exc}"))

        threading.Thread(target=work, daemon=True).start()

    def _show_initial_password(self) -> None:
        target = self._target_data_dir()
        candidates = [
            target / "admin.initial",
            ROOT / "data" / "admin.initial",
            ROOT / "data" / "dev" / "admin.initial",
        ]
        path = next((p for p in candidates if p.exists()), None)
        if path is None:
            messagebox.showinfo(
                "初始口令",
                f"没有找到 {target / 'admin.initial'}。\n\n"
                "如果面板已经初始化过，请使用你之前设置的管理员密码。\n"
                "忘记密码时，直接在上方输入新密码并点击「直接重置密码」即可，"
                "不需要旧密码。",
                parent=self.root,
            )
            return
        try:
            content = path.read_text(encoding="utf-8")
        except Exception as exc:
            messagebox.showerror("读取失败", str(exc), parent=self.root)
            return

        win = tk.Toplevel(self.root)
        win.title("初始管理员口令")
        win.configure(bg=CARD)
        win.transient(self.root)
        win.resizable(False, False)
        win.grab_set()
        frame = tk.Frame(win, bg=CARD)
        frame.pack(fill="both", expand=True, padx=18, pady=16)
        tk.Label(
            frame,
            text="首次启动生成的口令（登录后请立即修改并删除该文件）",
            bg=CARD,
            fg=TEXT,
            font=(FONT_UI, 10, "bold"),
        ).pack(anchor="w")
        tk.Label(
            frame,
            text=str(path),
            bg=CARD,
            fg=FAINT,
            font=(FONT_MONO, 8),
        ).pack(anchor="w", pady=(2, 8))
        text = tk.Text(
            frame,
            width=58,
            height=7,
            bg=LOG_BG,
            fg=LOG_FG,
            font=(FONT_MONO, 10),
            relief="flat",
            bd=0,
            padx=10,
            pady=8,
        )
        text.pack(fill="both", expand=True)
        text.insert("1.0", content)
        text.configure(state="disabled")
        btns = tk.Frame(frame, bg=CARD)
        btns.pack(fill="x", pady=(10, 0))

        def copy() -> None:
            win.clipboard_clear()
            win.clipboard_append(content)
            copy_btn.configure(text="已复制")

        copy_btn = self._button(btns, "复制", copy, "secondary")
        copy_btn.pack(side="left")
        self._button(btns, "关闭", win.destroy, "primary").pack(side="right")
        win.update_idletasks()
        x = self.root.winfo_rootx() + (self.root.winfo_width() - win.winfo_width()) // 2
        y = self.root.winfo_rooty() + (self.root.winfo_height() - win.winfo_height()) // 2
        win.geometry(f"+{max(0, x)}+{max(0, y)}")

    def _open_panel(self) -> None:
        if self._last_frontend_online:
            url = f"http://127.0.0.1:{self._active_frontend_port}"
        elif self._last_backend:
            url = f"http://127.0.0.1:{self._active_backend_port}"
        else:
            messagebox.showinfo(
                "面板未运行", "请先启动服务，或检查端口是否正确。", parent=self.root
            )
            return
        webbrowser.open(url)
        self._append_log("system", f"已在浏览器打开 {url}")

    def _open_backend(self) -> None:
        if not self._last_backend:
            messagebox.showinfo("后端未运行", "后端尚未就绪。", parent=self.root)
            return
        webbrowser.open(f"http://127.0.0.1:{self._active_backend_port}/api/health")

    def _open_data_dir(self) -> None:
        path = self._target_data_dir()
        try:
            if os.name == "nt":
                os.startfile(str(path))  # type: ignore[attr-defined]
            else:
                subprocess.Popen(["xdg-open", str(path)])
        except Exception as exc:
            messagebox.showerror("打开失败", str(exc), parent=self.root)

    # ------------------------------------------------------------------
    # 设置 / 退出
    # ------------------------------------------------------------------
    def _save_settings(self) -> None:
        try:
            bp = int(self.backend_port_var.get().strip())
            up = int(self.frontend_port_var.get().strip())
        except Exception:
            bp = self._active_backend_port
            up = self._active_frontend_port
        save_settings(
            {
                "backend_port": bp,
                "frontend_port": up,
                "username": self.username_var.get().strip() or "admin",
                "auto_frontend": bool(self.auto_frontend_var.get()),
                "auto_open": bool(self.auto_open_var.get()),
            }
        )

    def _on_close(self) -> None:
        if self._busy:
            if not messagebox.askyesno(
                "正在操作", "当前正在启动/停止服务，确定退出吗？", parent=self.root
            ):
                return
        owned = self._backend_proc is not None or self._frontend_proc is not None
        if owned:
            choice = messagebox.askyesnocancel(
                "退出",
                "服务仍在运行。\n\n"
                "是：停止服务并退出\n"
                "否：保持服务运行并退出\n"
                "取消：返回",
                parent=self.root,
            )
            if choice is None:
                return
            if choice:
                self._stop_blocking()
        self._closing = True
        self._save_settings()
        self.root.destroy()

    def run(self) -> None:
        self.root.mainloop()


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------
def selftest() -> int:
    print(f"{APP_NAME} v{APP_VERSION}")
    print(f"Python {sys.version.split()[0]} · Tk {tk.TkVersion}")
    print(f"项目目录: {ROOT}")
    print(f"设置文件: {SETTINGS_FILE}")
    print(f"node: {shutil.which('node') or '未找到'}")
    print(f"npm : {shutil.which('npm') or '未找到'}")
    print(f"cargo: {shutil.which('cargo') or '未找到'}")
    print(f"8787 占用: {port_in_use(8787)}")
    found = discover_backend(BACKEND_PORT_DEFAULT)
    print(f"发现后端: {found}")
    print(f"发现前端: {discover_frontend(FRONTEND_PORT_DEFAULT)}")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    if "--help" in sys.argv or "-h" in sys.argv:
        print(__doc__)
        return 0

    app = PanelGUI()
    if "--autostart" in sys.argv:
        app.root.after(700, app.start_all)
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
