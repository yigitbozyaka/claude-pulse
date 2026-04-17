"""
Claude Usage Monitor — Windows System Tray App
Fetches real usage from Anthropic OAuth API.
Monochrome terracotta design with Claude brand palette.
Auto-detects subscription tier (Pro, Max, etc.).
"""

import json
import os
import threading
import time
import tkinter as tk
import urllib.request
import urllib.error
from datetime import datetime, timedelta, timezone
from pathlib import Path
from collections import defaultdict

import pystray
from PIL import Image, ImageDraw, ImageFont, ImageFilter
import customtkinter as ctk


CLAUDE_DIR = Path.home() / ".claude"
CREDENTIALS_PATH = CLAUDE_DIR / ".credentials.json"
PROJECTS_DIR = CLAUDE_DIR / "projects"
CONFIG_PATH = Path(__file__).parent / "config.json"
USAGE_CACHE_PATH = Path(__file__).parent / ".usage_cache.json"

USAGE_API_URL = "https://api.anthropic.com/api/oauth/usage"
DEFAULT_CONFIG = {"refresh_interval_seconds": 60}

# ─── Claude Brand Palette ─────────────────────────────────────────────
#
#   bg:      #141413   — near-black warm
#   surface: #1c1c1a   — card bg
#   border:  #2a2926   — subtle warm edge
#   accent:  #c15f3c   — terracotta (primary & only color accent)
#   text:    #f4f3ee   — cream white
#   dim:     #b1ada1   — warm gray
#   muted:   #6b6860   — faded
#   track:   #252420   — gauge/bar track

BG         = "#141413"
SURFACE    = "#1c1c1a"
BORDER     = "#2a2926"
ACCENT     = "#c15f3c"
ACCENT_DIM = "#8b4532"    # muted terracotta for glow/secondary
TEXT       = "#f4f3ee"
DIM        = "#b1ada1"
MUTED      = "#6b6860"
TRACK      = "#252420"

# Model shades — monochrome variants of the palette, not rainbow
MODEL_SHADES = {
    "opus":   "#c15f3c",   # terracotta
    "sonnet": "#b1ada1",   # warm gray
    "haiku":  "#7a7568",   # darker gray
    "other":  "#4a4740",   # faded
}
MODEL_DISPLAY = {"opus": "Opus", "sonnet": "Sonnet", "haiku": "Haiku", "other": "Other"}

# ─── Subscription Detection ─────────────────────────────────────────

PLAN_DISPLAY = {
    "pro": "Pro", "max_5": "Max (5x)", "max_20": "Max (20x)",
    "free": "Free", "team": "Team", "max": "Max",
}
PLAN_MODELS = {
    "pro": ["Opus", "Haiku"],
    "max_5": ["Opus", "Sonnet", "Haiku"],
    "max_20": ["Opus", "Sonnet", "Haiku"],
    "max": ["Opus", "Sonnet", "Haiku"],
    "free": ["Haiku"],
    "team": ["Opus", "Sonnet", "Haiku"],
}

_account_info_cache = {"fetched": False, "plan": None}


def _normalize_plan(val):
    """Normalize various plan name formats to a standard key."""
    if not val: return None
    v = str(val).lower().strip().replace("-", "_").replace(" ", "_")
    if "max" in v and "20" in v: return "max_20"
    if "max" in v and "5" in v: return "max_5"
    if "max" in v: return "max"
    if "pro" in v: return "pro"
    if "free" in v: return "free"
    if "team" in v: return "team"
    return v


def _find_plan_in_dict(d, depth=0):
    """Recursively search a dict for plan/tier info, max 3 levels deep."""
    if not isinstance(d, dict) or depth > 3:
        return None
    for key in ("membershipTier", "membership_tier", "tier", "plan",
                "plan_type", "subscription_type"):
        val = d.get(key)
        if val and isinstance(val, str):
            return _normalize_plan(val)
    for key, val in d.items():
        if isinstance(val, dict):
            result = _find_plan_in_dict(val, depth + 1)
            if result:
                return result
    return None


def _try_fetch_account_info(token):
    """Try fetching subscription info from alternate API endpoints (once)."""
    if not token or _account_info_cache["fetched"]:
        return _account_info_cache.get("plan")
    _account_info_cache["fetched"] = True
    for url in [
        "https://api.anthropic.com/api/me",
        "https://api.anthropic.com/api/bootstrap",
    ]:
        try:
            req = urllib.request.Request(url, headers={
                "Authorization": f"Bearer {token}",
                "Accept": "application/json",
                "anthropic-beta": "oauth-2025-04-20",
                "User-Agent": "claude-code/2.1",
            })
            data = json.loads(urllib.request.urlopen(req, timeout=10).read().decode())
            plan = _find_plan_in_dict(data)
            if plan:
                _account_info_cache["plan"] = plan
                return plan
        except Exception:
            continue
    return None


def detect_subscription(data):
    """Detect subscription plan from usage API response with heuristic fallback."""
    if not data:
        return {"plan": "unknown", "display": "Claude", "has_sonnet": False, "models": []}

    # 1. Check usage API response for explicit plan/tier fields
    plan = _find_plan_in_dict(data)

    # 2. Try alternate account endpoints (lazy, cached)
    if not plan:
        token = get_oauth_token()
        plan = _try_fetch_account_info(token)

    # 3. Heuristic: sonnet window present → Max plan, absent → Pro
    has_sonnet = bool(data.get("seven_day_sonnet")
                      and isinstance(data.get("seven_day_sonnet"), dict)
                      and data["seven_day_sonnet"].get("resets_at"))
    if not plan:
        plan = "max" if has_sonnet else "pro"

    display = PLAN_DISPLAY.get(plan, plan.replace("_", " ").title())
    models = PLAN_MODELS.get(plan, ["Opus", "Haiku"])
    return {"plan": plan, "display": display, "has_sonnet": has_sonnet, "models": models}


# ─── Config / Credentials ────────────────────────────────────────────

def load_config():
    cfg = dict(DEFAULT_CONFIG)
    if CONFIG_PATH.exists():
        try:
            with open(CONFIG_PATH, "r") as f: cfg.update(json.load(f))
        except Exception: pass
    return cfg

def save_default_config():
    if not CONFIG_PATH.exists():
        with open(CONFIG_PATH, "w") as f: json.dump(DEFAULT_CONFIG, f, indent=2)

def get_oauth_token():
    try:
        with open(CREDENTIALS_PATH, "r", encoding="utf-8") as f:
            return json.load(f).get("claudeAiOauth", {}).get("accessToken")
    except Exception: return None


def refresh_oauth_token():
    """Use refresh token to get a new access token."""
    try:
        with open(CREDENTIALS_PATH, "r", encoding="utf-8") as f:
            creds = json.load(f)
        oauth = creds.get("claudeAiOauth", {})
        refresh_token = oauth.get("refreshToken")
        if not refresh_token:
            return None
        body = json.dumps({"grant_type": "refresh_token", "refresh_token": refresh_token}).encode()
        req = urllib.request.Request("https://api.anthropic.com/api/oauth/token", data=body, headers={
            "Content-Type": "application/json",
            "User-Agent": "claude-code/2.1",
        })
        resp = json.loads(urllib.request.urlopen(req, timeout=15).read().decode())
        if resp.get("access_token"):
            oauth["accessToken"] = resp["access_token"]
            if resp.get("refresh_token"):
                oauth["refreshToken"] = resp["refresh_token"]
            if resp.get("expires_in"):
                oauth["expiresAt"] = int(time.time() * 1000) + resp["expires_in"] * 1000
            creds["claudeAiOauth"] = oauth
            with open(CREDENTIALS_PATH, "w", encoding="utf-8") as f:
                json.dump(creds, f, indent=2)
            return resp["access_token"]
    except Exception:
        pass
    return None


# ─── Disk-backed cache + smart rate-limit handling ────────────────────

_last_usage_cache = {"data": None, "time": 0}
_rate_limit_state = {"backoff_until": 0, "consecutive_429s": 0}

# Minimum seconds between API calls
MIN_FETCH_INTERVAL = 55


def _save_cache_to_disk(data):
    try:
        with open(USAGE_CACHE_PATH, "w") as f:
            json.dump({"data": data, "time": time.time()}, f)
    except Exception:
        pass


def _load_cache_from_disk():
    try:
        with open(USAGE_CACHE_PATH, "r") as f:
            cached = json.load(f)
        if cached.get("data"):
            _last_usage_cache["data"] = cached["data"]
            _last_usage_cache["time"] = cached.get("time", 0)
            return cached["data"]
    except Exception:
        pass
    return None


def _do_api_call(token):
    """Single API call. Returns data or raises."""
    req = urllib.request.Request(USAGE_API_URL, headers={
        "Authorization": f"Bearer {token}",
        "Accept": "application/json",
        "Content-Type": "application/json",
        "anthropic-beta": "oauth-2025-04-20",
        "User-Agent": "claude-code/2.1",
    })
    data = json.loads(urllib.request.urlopen(req, timeout=15).read().decode())
    _last_usage_cache["data"] = data
    _last_usage_cache["time"] = time.time()
    _rate_limit_state["consecutive_429s"] = 0
    _save_cache_to_disk(data)
    return data


def fetch_usage_from_api():
    now = time.time()

    # Load disk cache on first run
    if _last_usage_cache["data"] is None:
        _load_cache_from_disk()

    # If we have recent cached data, don't hit the API at all
    if _last_usage_cache["data"] and (now - _last_usage_cache["time"]) < MIN_FETCH_INTERVAL:
        return _last_usage_cache["data"]

    # If we're in a rate-limit backoff period, return cached data
    if now < _rate_limit_state["backoff_until"]:
        return _last_usage_cache.get("data") or _fetch_usage_from_cli()

    token = get_oauth_token()
    if not token:
        return _fetch_usage_from_cli()

    try:
        return _do_api_call(token)
    except urllib.error.HTTPError as e:
        if e.code == 429:
            _rate_limit_state["consecutive_429s"] += 1
            # Exponential backoff: 60s, 120s, 240s, max 300s
            backoff = min(60 * (2 ** (_rate_limit_state["consecutive_429s"] - 1)), 300)
            _rate_limit_state["backoff_until"] = time.time() + backoff
        elif e.code == 401:
            new_token = refresh_oauth_token()
            if new_token:
                try:
                    return _do_api_call(new_token)
                except Exception:
                    pass
        # Return any cached data (memory or disk), fallback to CLI
        if _last_usage_cache.get("data"):
            return _last_usage_cache["data"]
        return _fetch_usage_from_cli()
    except Exception:
        if _last_usage_cache.get("data"):
            return _last_usage_cache["data"]
        return _fetch_usage_from_cli()


# ─── CLI Fallback ─────────────────────────────────────────────────────

def _fetch_usage_from_cli():
    """Last-resort fallback. On Windows, interactive /usage can't be piped,
    so this just returns any cached data we have."""
    return _last_usage_cache.get("data")

def parse_api_response(data):
    if not data: return None
    def pw(w):
        if not w: return {"utilization": 0, "resets_at": None}
        ra = None
        try: ra = datetime.fromisoformat(w.get("resets_at", ""))
        except Exception: pass
        return {"utilization": w.get("utilization", 0) or 0, "resets_at": ra}
    sub = detect_subscription(data)
    return {
        "session": pw(data.get("five_hour")),
        "weekly_all": pw(data.get("seven_day")),
        "weekly_sonnet": pw(data.get("seven_day_sonnet")),
        "weekly_opus": pw(data.get("seven_day_opus")),
        "extra_usage": data.get("extra_usage"),
        "subscription": sub,
    }


# ─── Local data for breakdown cards ──────────────────────────────────

def classify_model(s):
    if not s or s == "<synthetic>": return None
    m = s.lower()
    if "opus" in m: return "opus"
    if "sonnet" in m: return "sonnet"
    if "haiku" in m: return "haiku"
    return "other"

def parse_local_breakdown():
    now = datetime.now(timezone.utc)
    week_ago = now - timedelta(days=7)
    by_model = defaultdict(int)
    daily = defaultdict(lambda: defaultdict(int))
    tokens = {"input": 0, "output": 0, "requests": 0}
    for fpath in PROJECTS_DIR.glob("**/*.jsonl"):
        try:
            with open(fpath, "r", encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    try:
                        d = json.loads(line)
                        if d.get("type") != "assistant" or "message" not in d: continue
                        mc = classify_model(d["message"].get("model", ""))
                        if mc is None: continue
                        ts = datetime.fromisoformat(d.get("timestamp","").replace("Z","+00:00"))
                        if ts < week_ago: continue
                        by_model[mc] += 1
                        daily[ts.strftime("%Y-%m-%d")][mc] += 1
                        u = d["message"].get("usage", {})
                        tokens["input"] += u.get("input_tokens",0) + u.get("cache_creation_input_tokens",0) + u.get("cache_read_input_tokens",0)
                        tokens["output"] += u.get("output_tokens",0)
                        tokens["requests"] += 1
                    except Exception: continue
        except Exception: continue
    return {"by_model": dict(by_model), "daily": daily, "weekly_tokens": tokens}

def format_tokens(n):
    if n >= 1_000_000: return f"{n/1_000_000:.1f}M"
    if n >= 1_000: return f"{n/1_000:.1f}K"
    return str(n)

def time_until(dt):
    if not dt: return ""
    delta = dt - datetime.now(timezone.utc)
    if delta.total_seconds() <= 0: return "now"
    d, h, m = delta.days, delta.seconds//3600, (delta.seconds%3600)//60
    if d > 0: return f"{d}d {h}h"
    if h > 0: return f"{h}h {m}m"
    return f"{m}m"


# ─── Battery Tray Icon ───────────────────────────────────────────────

def _lerp_color(c1, c2, t):
    r1,g1,b1 = int(c1[1:3],16),int(c1[3:5],16),int(c1[5:7],16)
    r2,g2,b2 = int(c2[1:3],16),int(c2[3:5],16),int(c2[5:7],16)
    return f"#{int(r1+(r2-r1)*t):02x}{int(g1+(g2-g1)*t):02x}{int(b1+(b2-b1)*t):02x}"

def create_battery_icon(pct_used_100):
    """Vertical battery in Claude palette. Terracotta fill, cream text."""
    pct = max(0, min(100, pct_used_100)) / 100.0
    S = 128
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    bx, by, bw, bh = 28, 24, 72, 88
    cap_w, cap_h = 28, 10
    cap_x = bx + (bw - cap_w) // 2

    # Cap
    draw.rounded_rectangle([cap_x, by-cap_h+2, cap_x+cap_w, by+4],
                           radius=4, fill="#6b6860")
    # Shell
    draw.rounded_rectangle([bx, by, bx+bw, by+bh], radius=10,
                           fill="#1c1c1a", outline="#6b6860", width=3)

    fill_pct = 1.0 - pct
    fill_h = int((bh - 8) * fill_pct)

    if fill_h > 2:
        # Single color ramp: terracotta -> muted terracotta -> dark as it drains
        if pct <= 0.6:
            color = ACCENT                   # bright terracotta while healthy
        elif pct <= 0.85:
            color = _lerp_color(ACCENT, "#8b4532", (pct-0.6)/0.25)
        else:
            color = _lerp_color("#8b4532", "#5a2e1e", (pct-0.85)/0.15)

        ft = by + bh - 4 - fill_h
        fb = by + bh - 4
        draw.rounded_rectangle([bx+4, ft, bx+bw-4, fb], radius=6, fill=color)

        # Subtle glow
        glow = Image.new("RGBA", (S, S), (0,0,0,0))
        ImageDraw.Draw(glow).rounded_rectangle([bx+4, ft, bx+bw-4, fb], radius=6, fill=color)
        glow = glow.filter(ImageFilter.GaussianBlur(6))
        img = Image.alpha_composite(img, glow)
        ImageDraw.Draw(img).rounded_rectangle([bx+4, ft, bx+bw-4, fb], radius=6, fill=color)

    try: font = ImageFont.truetype("segoeuib.ttf", 26)
    except Exception:
        try: font = ImageFont.truetype("segoeui.ttf", 26)
        except Exception: font = ImageFont.load_default()

    ImageDraw.Draw(img).text((bx+bw//2, by+bh//2),
                              str(int(pct_used_100)), fill=TEXT, font=font, anchor="mm")
    return img.resize((64, 64), Image.LANCZOS)

def build_tooltip(usage):
    if not usage: return "Claude — loading..."
    sub = usage.get("subscription", {})
    plan_name = sub.get("display", "Claude")
    sp = usage["session"]["utilization"]
    wp = usage["weekly_all"]["utilization"]
    parts = [f"Session: {sp:.0f}%", f"Weekly: {wp:.0f}%"]
    if sub.get("has_sonnet"):
        snp = usage["weekly_sonnet"]["utilization"] if usage["weekly_sonnet"]["resets_at"] else 0
        parts.append(f"Sonnet: {snp:.0f}%")
    return (f"Claude {plan_name}\n"
            f"{'  |  '.join(parts)}\n"
            f"Resets in {time_until(usage['weekly_all']['resets_at'])}")


# ─── PIL-rendered Arc Gauge (anti-aliased) ──────────────────────────

import math

def _hex_to_rgb(h):
    return (int(h[1:3],16), int(h[3:5],16), int(h[5:7],16))

def _blend_rgb(fg, bg, a):
    return tuple(int(f*a + b*(1-a)) for f, b in zip(fg, bg))

def render_gauge_image(width, height, gauges):
    """Render all gauges into a single PIL image at 2x for anti-aliasing.
    gauges: list of (cx, cy, radius, thickness, pct, label, sub, val_text)
    """
    S = 2  # supersampling factor
    W, H = width * S, height * S
    bg_rgb = _hex_to_rgb(BG)
    img = Image.new("RGBA", (W, H), (*bg_rgb, 255))
    draw = ImageDraw.Draw(img)

    accent_rgb = _hex_to_rgb(ACCENT)
    track_rgb = _hex_to_rgb(TRACK)
    text_rgb = _hex_to_rgb(TEXT)
    dim_rgb = _hex_to_rgb(DIM)
    muted_rgb = _hex_to_rgb(MUTED)
    glow_rgb = _blend_rgb(accent_rgb, bg_rgb, 0.18)

    try: font_big = ImageFont.truetype("segoeuib.ttf", 22 * S)
    except Exception: font_big = ImageFont.load_default()
    try: font_med = ImageFont.truetype("segoeui.ttf", 10 * S)
    except Exception: font_med = ImageFont.load_default()
    try: font_sm = ImageFont.truetype("segoeui.ttf", 8 * S)
    except Exception: font_sm = ImageFont.load_default()

    for (cx, cy, radius, thickness, pct, label, sub, val_text) in gauges:
        cx2, cy2, r, t = cx*S, cy*S, radius*S, thickness*S
        start_angle, sweep_angle = 135, 270

        # Draw arc track
        _draw_thick_arc(draw, cx2, cy2, r, t, start_angle, sweep_angle, track_rgb)

        # Draw value arc with glow
        sv = sweep_angle * min(pct, 1.0)
        if sv > 0.5:
            _draw_thick_arc(draw, cx2, cy2, r, t+10*S, start_angle, sv, (*glow_rgb, 50))
            _draw_thick_arc(draw, cx2, cy2, r, t, start_angle, sv, accent_rgb)

        # Center text
        draw.text((cx2, cy2 - 10*S), val_text, fill=text_rgb, font=font_big, anchor="mm")
        draw.text((cx2, cy2 + 16*S), label, fill=dim_rgb, font=font_med, anchor="mm")
        draw.text((cx2, cy2 + 32*S), sub, fill=muted_rgb, font=font_sm, anchor="mm")

    # Downsample for smooth anti-aliasing
    return img.resize((width, height), Image.LANCZOS)


def _draw_thick_arc(draw, cx, cy, r, thickness, start_deg, sweep_deg, color):
    """Draw a thick arc using a polygon for smooth anti-aliased rendering."""
    if sweep_deg < 0.1:
        return
    steps = max(int(sweep_deg / 2), 20)
    half_t = thickness / 2
    r_out = r + half_t
    r_in = r - half_t

    points_outer = []
    points_inner = []
    for i in range(steps + 1):
        frac = i / steps
        angle_deg = start_deg + sweep_deg * frac
        angle_rad = math.radians(angle_deg)
        cos_a, sin_a = math.cos(angle_rad), -math.sin(angle_rad)
        points_outer.append((cx + r_out * cos_a, cy + r_out * sin_a))
        points_inner.append((cx + r_in * cos_a, cy + r_in * sin_a))

    poly = points_outer + list(reversed(points_inner))
    if len(color) == 4:
        # RGBA — draw on temporary layer
        tmp = Image.new("RGBA", draw.im.size, (0, 0, 0, 0))
        ImageDraw.Draw(tmp).polygon(poly, fill=color)
        draw._image.alpha_composite(tmp)  # type: ignore
    else:
        draw.polygon(poly, fill=color)


# ─── Dashboard Window ────────────────────────────────────────────────

class DashboardWindow:
    def __init__(self, data_getter):
        self.data_getter = data_getter
        self.window = None
        self._building = False
        self._refreshing = False
        self._anim_target = {}
        self._anim_current = {}
        self._content_frame = None
        self._overlay = None
        self._spinner_angle = 0
        self._auto_refresh_id = None

    def show(self):
        if self._building: return
        if self.window is not None:
            try: self.window.focus(); return
            except Exception: self.window = None
        self._building = True
        threading.Thread(target=self._build, daemon=True).start()

    def _build(self):
        try:
            usage, local = self.data_getter()
            ctk.set_appearance_mode("dark")

            self.window = ctk.CTk()
            self.window.title("Claude Usage")
            self.window.geometry("580x820")
            self.window.resizable(False, False)
            self.window.configure(fg_color=BG)
            try:
                ico = Path(__file__).parent / "logo.ico"
                if ico.exists():
                    self.window.iconbitmap(str(ico))
            except Exception: pass
            self.window.protocol("WM_DELETE_WINDOW", self._on_close)

            # Persistent outer container - never destroyed
            self._outer = ctk.CTkFrame(self.window, fg_color=BG)
            self._outer.pack(fill="both", expand=True)

            self._populate(usage, local)

            self._building = False
            self.window.mainloop()
        except Exception:
            self._building = False
            import traceback; traceback.print_exc()

    def _populate(self, usage, local):
        """Build or rebuild all content inside the window."""
        # Destroy old scrollable frame's children cleanly
        if self._content_frame:
            self._content_frame.pack_forget()
            self._content_frame.destroy()
            self._content_frame = None

        main = ctk.CTkScrollableFrame(self._outer, fg_color="transparent",
                                      scrollbar_button_color=BORDER,
                                      scrollbar_button_hover_color=MUTED)
        main.pack(fill="both", expand=True)
        self._content_frame = main

        # ── Header with refresh button ──
        hdr = ctk.CTkFrame(main, fg_color="transparent")
        hdr.pack(fill="x", padx=32, pady=(28, 0))
        ctk.CTkLabel(hdr, text="Usage",
                     font=ctk.CTkFont(family="Segoe UI Light", size=32),
                     text_color=TEXT).pack(side="left")

        right = ctk.CTkFrame(hdr, fg_color="transparent")
        right.pack(side="right")
        self._refresh_btn = ctk.CTkButton(
            right, text="Refresh", command=self._refresh,
            fg_color=SURFACE, hover_color=BORDER,
            font=ctk.CTkFont(size=11),
            height=30, width=80, corner_radius=6,
            text_color=DIM, border_width=1, border_color=BORDER)
        self._refresh_btn.pack(side="left", padx=(0, 8), pady=6)
        sub = usage.get("subscription", {}) if usage else {}
        plan_label = sub.get("display", "Claude")
        pill = ctk.CTkFrame(right, fg_color=SURFACE, corner_radius=6,
                            border_width=1, border_color=BORDER)
        pill.pack(side="left", pady=6)
        ctk.CTkLabel(pill, text=f"  {plan_label}  ",
                     font=ctk.CTkFont(size=11), text_color=ACCENT).pack(padx=2, pady=3)

        if not usage:
            ctk.CTkLabel(main, text="Could not fetch usage data.\nWill retry automatically.",
                         font=ctk.CTkFont(size=13), text_color=DIM).pack(pady=50)
            self._schedule_auto_refresh(10000)
            return

        # ── Subscription info ──
        self._build_subscription_card(main, usage)

        # ── Gauges ──
        self._build_gauges(main, usage)

        # ── Thin rule ──
        ctk.CTkFrame(main, height=1, fg_color=BORDER).pack(fill="x", padx=32, pady=(2, 0))

        # ── Progress bars ──
        self._build_limits(main, usage)

        # ── Thin rule ──
        ctk.CTkFrame(main, height=1, fg_color=BORDER).pack(fill="x", padx=32, pady=(12, 16))

        # ── Cards ──
        if local:
            self._build_model_card(main, local)
            self._build_daily_card(main, local)
            self._build_token_card(main, local)

        # Auto-refresh every 60s while window is open
        self._schedule_auto_refresh(60000)

    def _schedule_auto_refresh(self, ms):
        """Schedule a silent auto-refresh. Cancels any previous timer."""
        if self._auto_refresh_id:
            try: self.window.after_cancel(self._auto_refresh_id)
            except Exception: pass
        try:
            self._auto_refresh_id = self.window.after(ms, self._auto_refresh)
        except Exception:
            pass

    def _auto_refresh(self):
        """Silent refresh - no spinner, just swap content."""
        if not self.window or self._refreshing:
            return
        def do_fetch():
            usage, local = self.data_getter()
            try:
                self.window.after(0, lambda: self._finish_auto_refresh(usage, local))
            except Exception: pass
        threading.Thread(target=do_fetch, daemon=True).start()

    def _finish_auto_refresh(self, usage, local):
        if usage:
            self._populate(usage, local)

    # ── Subscription card ──

    def _build_subscription_card(self, parent, usage):
        sub = usage.get("subscription", {})
        plan = sub.get("plan", "unknown")
        display = sub.get("display", "Claude")
        models = sub.get("models", [])

        card = ctk.CTkFrame(parent, fg_color=SURFACE, corner_radius=12,
                            border_width=1, border_color=BORDER)
        card.pack(fill="x", padx=32, pady=(16, 0))

        row = ctk.CTkFrame(card, fg_color="transparent")
        row.pack(fill="x", padx=18, pady=(12, 4))
        ctk.CTkLabel(row, text="Subscription",
                     font=ctk.CTkFont(size=13, weight="bold"),
                     text_color=TEXT).pack(side="left")
        plan_pill = ctk.CTkFrame(row, fg_color=ACCENT, corner_radius=4)
        plan_pill.pack(side="right")
        ctk.CTkLabel(plan_pill, text=f" {display} ",
                     font=ctk.CTkFont(size=10, weight="bold"),
                     text_color=TEXT).pack(padx=4, pady=2)

        if models:
            model_row = ctk.CTkFrame(card, fg_color="transparent")
            model_row.pack(fill="x", padx=18, pady=(0, 10))
            ctk.CTkLabel(model_row, text="Models",
                         font=ctk.CTkFont(size=10), text_color=MUTED).pack(side="left", padx=(0, 8))
            for m in models:
                badge = ctk.CTkFrame(model_row, fg_color=TRACK, corner_radius=4,
                                     border_width=1, border_color=BORDER)
                badge.pack(side="left", padx=2)
                ctk.CTkLabel(badge, text=f" {m} ",
                             font=ctk.CTkFont(size=9), text_color=DIM).pack(padx=2, pady=1)

    # ── Gauges ──

    def _build_gauges(self, parent, usage):
        frame = ctk.CTkFrame(parent, fg_color="transparent", height=185)
        frame.pack(fill="x", padx=8, pady=(20, 8))
        frame.pack_propagate(False)

        self._gauge_label = tk.Label(frame, bg=BG, borderwidth=0)
        self._gauge_label.pack(fill="both", expand=True)

        sp = usage["session"]["utilization"] / 100.0
        wp = usage["weekly_all"]["utilization"] / 100.0

        rs = time_until(usage["session"]["resets_at"])
        rw = time_until(usage["weekly_all"]["resets_at"])

        r, t = 64, 9
        yc = 90

        has_sonnet = usage.get("subscription", {}).get("has_sonnet", False)

        if has_sonnet:
            sn = (usage["weekly_sonnet"]["utilization"]/100.0) if usage["weekly_sonnet"]["resets_at"] else 0
            self._anim_target = {"s": sp, "w": wp, "n": sn}
            self._anim_current = {"s": 0.0, "w": 0.0, "n": 0.0}
            self._gauge_keys = ["s", "w", "n"]
            self._gm = {
                "s": (98,  yc, r, t, "Session",    f"resets {rs}"),
                "w": (288, yc, r, t, "All Models", f"resets {rw}"),
                "n": (478, yc, r, t, "Sonnet",     f"resets {rw}"),
            }
        else:
            self._anim_target = {"s": sp, "w": wp}
            self._anim_current = {"s": 0.0, "w": 0.0}
            self._gauge_keys = ["s", "w"]
            self._gm = {
                "s": (160, yc, r, t, "Session",    f"resets {rs}"),
                "w": (400, yc, r, t, "All Models", f"resets {rw}"),
            }

        self._gauge_w, self._gauge_h = 560, 180
        self._animate()

    def _animate(self):
        done = True
        gauges = []
        for k in self._gauge_keys:
            cx, cy, r, t, lbl, sub = self._gm[k]
            tgt = self._anim_target[k]
            cur = self._anim_current[k]
            if abs(tgt - cur) > 0.002:
                cur += (tgt - cur) * 0.10
                self._anim_current[k] = cur
                done = False
            else:
                cur = tgt; self._anim_current[k] = cur
            gauges.append((cx, cy, r, t, cur, lbl, sub, f"{int(cur*100)}%"))

        pil_img = render_gauge_image(self._gauge_w, self._gauge_h, gauges)
        from PIL import ImageTk
        self._gauge_photo = ImageTk.PhotoImage(pil_img)
        self._gauge_label.configure(image=self._gauge_photo)

        if not done:
            try: self.window.after(16, self._animate)
            except Exception: pass

    # ── Progress bar section ──

    def _build_limits(self, parent, usage):
        frame = ctk.CTkFrame(parent, fg_color="transparent")
        frame.pack(fill="x", padx=32, pady=(14, 0))

        sp = usage["session"]["utilization"]
        wp = usage["weekly_all"]["utilization"]
        rs = time_until(usage["session"]["resets_at"])
        rw = time_until(usage["weekly_all"]["resets_at"])

        self._bar_row(frame, "Current session", f"Resets in {rs}", sp)
        self._bar_row(frame, "All models",      f"Resets in {rw}", wp)
        if usage.get("subscription", {}).get("has_sonnet") and usage["weekly_sonnet"]["resets_at"]:
            sn = usage["weekly_sonnet"]["utilization"]
            self._bar_row(frame, "Sonnet only",  f"Resets in {rw}", sn)

        # Show cache age if data is stale
        cache_age = time.time() - _last_usage_cache.get("time", time.time())
        now_s = datetime.now().strftime("%H:%M:%S")
        if cache_age > 120:
            age_m = int(cache_age // 60)
            status = f"Updated {now_s}  ·  data from {age_m}m ago (rate-limited)"
            color = ACCENT_DIM
        elif _rate_limit_state.get("consecutive_429s", 0) > 0:
            status = f"Updated {now_s}  ·  cached (rate-limited, backing off)"
            color = ACCENT_DIM
        else:
            status = f"Updated {now_s}"
            color = MUTED
        ctk.CTkLabel(frame, text=status,
                     font=ctk.CTkFont(size=9), text_color=color).pack(anchor="w", pady=(10, 0))

    def _bar_row(self, parent, title, subtitle, pct_100):
        row = ctk.CTkFrame(parent, fg_color="transparent")
        row.pack(fill="x", pady=(8, 0))

        ctk.CTkLabel(row, text=title, font=ctk.CTkFont(size=13, weight="bold"),
                     text_color=TEXT).pack(side="left", anchor="w")
        ctk.CTkLabel(row, text=f"{pct_100:.0f}% used",
                     font=ctk.CTkFont(size=12), text_color=DIM).pack(side="right")

        sub_row = ctk.CTkFrame(parent, fg_color="transparent")
        sub_row.pack(fill="x")
        ctk.CTkLabel(sub_row, text=subtitle,
                     font=ctk.CTkFont(size=10), text_color=MUTED).pack(side="left")

        # Bar
        bar = ctk.CTkFrame(parent, fg_color=TRACK, corner_radius=4, height=8)
        bar.pack(fill="x", pady=(4, 0))
        bar.pack_propagate(False)
        p = pct_100 / 100.0
        if p > 0.005:
            ctk.CTkFrame(bar, fg_color=ACCENT, corner_radius=4,
                         width=max(int(p * 500), 5)).pack(side="left", fill="y")

    # ── Model card ──

    def _build_model_card(self, parent, local):
        card = ctk.CTkFrame(parent, fg_color=SURFACE, corner_radius=12,
                            border_width=1, border_color=BORDER)
        card.pack(fill="x", padx=32, pady=(0, 10))

        ctk.CTkLabel(card, text="Model distribution",
                     font=ctk.CTkFont(size=14, weight="bold"),
                     text_color=TEXT).pack(anchor="w", padx=18, pady=(14, 8))

        bm = local["by_model"]
        total = max(sum(bm.values()), 1)

        bar = ctk.CTkFrame(card, fg_color=TRACK, corner_radius=5, height=20)
        bar.pack(fill="x", padx=18, pady=(0, 6))
        bar.pack_propagate(False)
        for m in ["opus", "sonnet", "haiku", "other"]:
            c = bm.get(m, 0)
            if c == 0: continue
            w = max(int((c/total)*490), 4)
            ctk.CTkFrame(bar, fg_color=MODEL_SHADES[m], corner_radius=4,
                         width=w).pack(side="left", fill="y", padx=1, pady=2)

        legend = ctk.CTkFrame(card, fg_color="transparent")
        legend.pack(fill="x", padx=18, pady=(0, 14))
        for m in ["opus", "sonnet", "haiku", "other"]:
            c = bm.get(m, 0)
            if c == 0: continue
            item = ctk.CTkFrame(legend, fg_color="transparent")
            item.pack(side="left", padx=(0, 16))
            ctk.CTkFrame(item, width=8, height=8, corner_radius=4,
                         fg_color=MODEL_SHADES[m]).pack(side="left", padx=(0, 5))
            ctk.CTkLabel(item, text=f"{MODEL_DISPLAY[m]} {c/total*100:.0f}%",
                         font=ctk.CTkFont(size=10), text_color=DIM).pack(side="left")

    # ── Daily activity ──

    def _build_daily_card(self, parent, local):
        card = ctk.CTkFrame(parent, fg_color=SURFACE, corner_radius=12,
                            border_width=1, border_color=BORDER)
        card.pack(fill="x", padx=32, pady=(0, 10))

        ctk.CTkLabel(card, text="Daily activity",
                     font=ctk.CTkFont(size=14, weight="bold"),
                     text_color=TEXT).pack(anchor="w", padx=18, pady=(14, 8))

        daily = local["daily"]
        now = datetime.now(timezone.utc)
        days = [(now - timedelta(days=i)).strftime("%Y-%m-%d") for i in range(6, -1, -1)]
        totals = [sum(daily.get(d, {}).values()) for d in days]
        max_c = max(totals) or 1
        day_labels = [datetime.strptime(d, "%Y-%m-%d").strftime("%a") for d in days]

        # Render chart with PIL for clean anti-aliased bars
        cw, ch = 480, 110
        S = 2
        img = Image.new("RGBA", (cw*S, ch*S), _hex_to_rgb(SURFACE) + (255,))
        draw = ImageDraw.Draw(img)

        accent_rgb = _hex_to_rgb(ACCENT)
        muted_rgb = _hex_to_rgb(MUTED)
        track_rgb = _hex_to_rgb(TRACK)
        try: font_count = ImageFont.truetype("segoeui.ttf", 9*S)
        except Exception: font_count = ImageFont.load_default()
        try: font_day = ImageFont.truetype("segoeui.ttf", 10*S)
        except Exception: font_day = ImageFont.load_default()

        bar_area_h = 65 * S
        label_area = 30 * S
        top_pad = 5 * S
        n = len(days)
        col_w = (cw * S) // n

        for i, (total, label) in enumerate(zip(totals, day_labels)):
            x_center = i * col_w + col_w // 2
            bar_w = col_w - 12 * S

            if total > 0:
                bh = max(int((total / max_c) * bar_area_h), 4*S)
                x1 = x_center - bar_w // 2
                y1 = top_pad + bar_area_h - bh
                x2 = x_center + bar_w // 2
                y2 = top_pad + bar_area_h
                # Rounded bar
                r = min(4*S, bar_w//2, bh//2)
                draw.rounded_rectangle([x1, y1, x2, y2], radius=r, fill=accent_rgb)
                # Count label
                draw.text((x_center, y2 + 3*S), str(total), fill=muted_rgb,
                          font=font_count, anchor="mt")
            else:
                # Faint placeholder bar
                x1 = x_center - bar_w // 2
                y2 = top_pad + bar_area_h
                y1 = y2 - 3*S
                draw.rounded_rectangle([x1, y1, x1+bar_w, y2], radius=2*S, fill=track_rgb)

            # Day label
            draw.text((x_center, ch*S - 4*S), label, fill=muted_rgb,
                      font=font_day, anchor="mb")

        chart_img = img.resize((cw, ch), Image.LANCZOS)
        from PIL import ImageTk
        self._daily_photo = ImageTk.PhotoImage(chart_img)
        chart_label = tk.Label(card, image=self._daily_photo, bg=SURFACE, borderwidth=0)
        chart_label.pack(padx=18, pady=(0, 14))

    # ── Token stats ──

    def _build_token_card(self, parent, local):
        card = ctk.CTkFrame(parent, fg_color=SURFACE, corner_radius=12,
                            border_width=1, border_color=BORDER)
        card.pack(fill="x", padx=32, pady=(0, 10))

        ctk.CTkLabel(card, text="Token usage this week",
                     font=ctk.CTkFont(size=14, weight="bold"),
                     text_color=TEXT).pack(anchor="w", padx=18, pady=(14, 8))

        wt = local["weekly_tokens"]
        grid = ctk.CTkFrame(card, fg_color="transparent")
        grid.pack(fill="x", padx=18, pady=(0, 14))

        for lbl, val in [
            ("Input",    format_tokens(wt["input"])),
            ("Output",   format_tokens(wt["output"])),
            ("Requests", str(wt["requests"])),
        ]:
            box = ctk.CTkFrame(grid, fg_color=TRACK, corner_radius=8,
                               border_width=1, border_color=BORDER)
            box.pack(side="left", fill="both", expand=True, padx=3)
            ctk.CTkLabel(box, text=val,
                         font=ctk.CTkFont(size=18, weight="bold"),
                         text_color=TEXT).pack(padx=10, pady=(10, 1))
            ctk.CTkLabel(box, text=lbl,
                         font=ctk.CTkFont(size=9), text_color=MUTED).pack(padx=10, pady=(0, 10))

    def _refresh(self):
        if self._refreshing or not self.window:
            return
        self._refreshing = True
        self._refresh_btn.configure(state="disabled", text_color=MUTED)
        self._show_loading()

        def do_fetch():
            usage, local = self.data_getter()
            try:
                self.window.after(0, lambda: self._finish_refresh(usage, local))
            except Exception:
                pass

        threading.Thread(target=do_fetch, daemon=True).start()

    def _show_loading(self):
        """Show a semi-transparent overlay with a spinning arc."""
        self._overlay = tk.Canvas(self._outer, bg=BG, highlightthickness=0)
        self._overlay.place(relx=0, rely=0, relwidth=1, relheight=1)
        # Darken effect
        self._overlay.create_rectangle(0, 0, 600, 900, fill=BG, stipple="gray50")
        self._spinner_angle = 0
        self._spin_loading()

    def _spin_loading(self):
        if not self._overlay:
            return
        c = self._overlay
        c.delete("spinner")
        cx, cy, r = 290, 400, 20
        a = self._spinner_angle
        c.create_arc(cx-r, cy-r, cx+r, cy+r, start=a, extent=90,
                     style="arc", outline=ACCENT, width=3, tags="spinner")
        c.create_text(cx, cy+35, text="Refreshing...", fill=DIM,
                      font=("Segoe UI", 11), tags="spinner")
        self._spinner_angle = (a + 15) % 360
        try:
            self.window.after(30, self._spin_loading)
        except Exception:
            pass

    def _finish_refresh(self, usage, local):
        """Remove overlay and rebuild content."""
        if self._overlay:
            self._overlay.destroy()
            self._overlay = None
        self._populate(usage, local)
        self._refreshing = False

    def _on_close(self):
        if self._overlay:
            self._overlay.destroy()
            self._overlay = None
        if self.window:
            self.window.destroy()
            self.window = None


# ─── Main Monitor ─────────────────────────────────────────────────────

class ClaudeUsageMonitor:
    def __init__(self):
        self.cfg = load_config(); save_default_config()
        self.usage = None; self.local = None
        self.dashboard = DashboardWindow(self._get_fresh)
        self.icon = None; self._refresh()

    def _get_fresh(self):
        self._refresh(); return self.usage, self.local

    def _refresh(self):
        self.cfg = load_config()
        raw = fetch_usage_from_api()
        if raw: self.usage = parse_api_response(raw)
        self.local = parse_local_breakdown()

    def _session_pct(self):
        return self.usage["session"]["utilization"] if self.usage else 0

    def _update_loop(self):
        while True:
            time.sleep(self.cfg.get("refresh_interval_seconds", 30))
            try:
                self._refresh()
                if self.icon and self.usage:
                    self.icon.icon = create_battery_icon(self._session_pct())
                    self.icon.title = build_tooltip(self.usage)
            except Exception: pass

    def _on_open(self, icon, item): self.dashboard.show()
    def _on_refresh(self, icon, item):
        self._refresh()
        if self.icon and self.usage:
            self.icon.icon = create_battery_icon(self._session_pct())
            self.icon.title = build_tooltip(self.usage)
    def _on_quit(self, icon, item): icon.stop()

    def run(self):
        menu = pystray.Menu(
            pystray.MenuItem("Open Dashboard", self._on_open, default=True),
            pystray.MenuItem("Refresh", self._on_refresh),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Quit", self._on_quit),
        )
        self.icon = pystray.Icon("claude_usage",
                                  create_battery_icon(self._session_pct()),
                                  build_tooltip(self.usage), menu)
        threading.Thread(target=self._update_loop, daemon=True).start()
        self.icon.run()


if __name__ == "__main__":
    ClaudeUsageMonitor().run()
