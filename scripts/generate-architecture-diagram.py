#!/usr/bin/env python3
"""Generate architecture diagram for ColotCook README."""

from PIL import Image, ImageDraw, ImageFont
import os

WIDTH = 1200
HEIGHT = 720
BG = "#0d1117"
CARD_BG = "#161b22"
BORDER = "#30363d"
ACCENT = "#58a6ff"
GREEN = "#3fb950"
ORANGE = "#d29922"
PURPLE = "#bc8cff"
RED = "#f85149"
CYAN = "#39d353"
WHITE = "#e6edf3"
GRAY = "#8b949e"

def hex_to_rgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))

def draw_rounded_rect(draw, xy, radius, fill, outline=None):
    x0, y0, x1, y1 = xy
    draw.rounded_rectangle(xy, radius=radius, fill=fill, outline=outline)

def main():
    img = Image.new("RGB", (WIDTH, HEIGHT), hex_to_rgb(BG))
    draw = ImageDraw.Draw(img)

    try:
        title_font = ImageFont.truetype("/System/Library/Fonts/SFCompact.ttf", 28)
        header_font = ImageFont.truetype("/System/Library/Fonts/SFCompact.ttf", 18)
        body_font = ImageFont.truetype("/System/Library/Fonts/SFCompact.ttf", 14)
        small_font = ImageFont.truetype("/System/Library/Fonts/SFCompact.ttf", 12)
    except (OSError, IOError):
        try:
            title_font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 28)
            header_font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 18)
            body_font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 14)
            small_font = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 12)
        except (OSError, IOError):
            title_font = ImageFont.load_default()
            header_font = title_font
            body_font = title_font
            small_font = title_font

    # Title
    draw.text((WIDTH // 2, 30), "ColotCook Architecture", fill=hex_to_rgb(WHITE),
              font=title_font, anchor="mt")
    draw.text((WIDTH // 2, 62), "7 Crates · 40+ Modules · Pure Rust",
              fill=hex_to_rgb(GRAY), font=small_font, anchor="mt")

    # === TOP ROW: CLI ===
    cli_x, cli_y = 40, 90
    cli_w, cli_h = WIDTH - 80, 100
    draw_rounded_rect(draw, (cli_x, cli_y, cli_x + cli_w, cli_y + cli_h),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(ACCENT))
    draw.text((cli_x + 20, cli_y + 12), "colotcook-cli", fill=hex_to_rgb(ACCENT), font=header_font)
    draw.text((cli_x + 200, cli_y + 14), "Binary Entry Point & Terminal UI",
              fill=hex_to_rgb(GRAY), font=small_font)

    modules = ["main", "arg_parsing", "live_cli", "streaming", "runtime_build",
               "reports", "session_mgmt", "oauth_flow", "tool_display", "util", "render"]
    mx = cli_x + 20
    for m in modules:
        tw = body_font.getlength(m) + 16
        draw_rounded_rect(draw, (mx, cli_y + 45, mx + tw, cli_y + 75),
                          6, hex_to_rgb(BG), hex_to_rgb(BORDER))
        draw.text((mx + 8, cli_y + 50), m, fill=hex_to_rgb(WHITE), font=body_font)
        mx += tw + 8

    # === MIDDLE ROW: Commands, Tools ===
    row2_y = 210

    # Commands
    cmd_x, cmd_w = 40, 340
    draw_rounded_rect(draw, (cmd_x, row2_y, cmd_x + cmd_w, row2_y + 130),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(GREEN))
    draw.text((cmd_x + 20, row2_y + 12), "colotcook-commands", fill=hex_to_rgb(GREEN), font=header_font)
    draw.text((cmd_x + 20, row2_y + 38), "15 Slash Commands", fill=hex_to_rgb(GRAY), font=small_font)
    cmods = ["types", "validation", "help", "handlers", "agents_skills", "plugins_cmd"]
    cy = row2_y + 58
    for i, m in enumerate(cmods):
        col = cmd_x + 20 + (i % 3) * 108
        row = cy + (i // 3) * 28
        draw_rounded_rect(draw, (col, row, col + 100, row + 22),
                          4, hex_to_rgb(BG), hex_to_rgb(BORDER))
        draw.text((col + 6, row + 3), m, fill=hex_to_rgb(WHITE), font=small_font)

    # Tools
    tool_x = 400
    tool_w = WIDTH - 40 - tool_x
    draw_rounded_rect(draw, (tool_x, row2_y, tool_x + tool_w, row2_y + 130),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(ORANGE))
    draw.text((tool_x + 20, row2_y + 12), "colotcook-tools", fill=hex_to_rgb(ORANGE), font=header_font)
    draw.text((tool_x + 20, row2_y + 38), "19 Built-in Tools", fill=hex_to_rgb(GRAY), font=small_font)
    tmods = ["file_tools", "search_tools", "web_tools", "execution_tools",
             "agent_tools", "session_tools", "system_tools", "types"]
    ty = row2_y + 58
    for i, m in enumerate(tmods):
        col = tool_x + 20 + (i % 4) * 190
        row = ty + (i // 4) * 28
        draw_rounded_rect(draw, (col, row, col + 180, row + 22),
                          4, hex_to_rgb(BG), hex_to_rgb(BORDER))
        draw.text((col + 6, row + 3), m, fill=hex_to_rgb(WHITE), font=small_font)

    # === ROW 3: Runtime, Plugins ===
    row3_y = 360

    # Runtime
    rt_x, rt_w = 40, 700
    draw_rounded_rect(draw, (rt_x, row3_y, rt_x + rt_w, row3_y + 130),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(PURPLE))
    draw.text((rt_x + 20, row3_y + 12), "colotcook-runtime", fill=hex_to_rgb(PURPLE), font=header_font)
    draw.text((rt_x + 250, row3_y + 14), "Agent Loop · Sessions · Sandbox · Permissions",
              fill=hex_to_rgb(GRAY), font=small_font)
    rmods = ["conversation", "session", "config", "permissions", "sandbox",
             "mcp_stdio", "mcp_http", "hooks", "prompt", "oauth", "compact",
             "file_ops", "bash", "json", "logging"]
    ry = row3_y + 42
    for i, m in enumerate(rmods):
        col = rt_x + 20 + (i % 5) * 138
        row = ry + (i // 5) * 28
        draw_rounded_rect(draw, (col, row, col + 130, row + 22),
                          4, hex_to_rgb(BG), hex_to_rgb(BORDER))
        draw.text((col + 6, row + 3), m, fill=hex_to_rgb(WHITE), font=small_font)

    # Plugins
    pl_x = 760
    pl_w = WIDTH - 40 - pl_x
    draw_rounded_rect(draw, (pl_x, row3_y, pl_x + pl_w, row3_y + 130),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(RED))
    draw.text((pl_x + 20, row3_y + 12), "colotcook-plugins", fill=hex_to_rgb(RED), font=header_font)
    draw.text((pl_x + 20, row3_y + 38), "Plugin Lifecycle & Hooks", fill=hex_to_rgb(GRAY), font=small_font)
    pmods = ["types", "registry", "discovery", "lifecycle", "hooks"]
    py = row3_y + 58
    for i, m in enumerate(pmods):
        col = pl_x + 20 + (i % 2) * 190
        row = py + (i // 2) * 28
        tw = small_font.getlength(m) + 12
        draw_rounded_rect(draw, (col, row, col + tw, row + 22),
                          4, hex_to_rgb(BG), hex_to_rgb(BORDER))
        draw.text((col + 6, row + 3), m, fill=hex_to_rgb(WHITE), font=small_font)

    # === BOTTOM ROW: API, Telemetry ===
    row4_y = 510

    # API
    api_x, api_w = 40, 780
    draw_rounded_rect(draw, (api_x, row4_y, api_x + api_w, row4_y + 110),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(CYAN))
    draw.text((api_x + 20, row4_y + 12), "colotcook-api", fill=hex_to_rgb(CYAN), font=header_font)
    draw.text((api_x + 200, row4_y + 14), "Provider Abstraction & Streaming",
              fill=hex_to_rgb(GRAY), font=small_font)

    providers = [
        ("Anthropic", ACCENT), ("OpenAI", GREEN), ("Gemini", ORANGE),
        ("xAI", PURPLE), ("Ollama", RED)
    ]
    px = api_x + 20
    for name, color in providers:
        tw = header_font.getlength(name) + 24
        draw_rounded_rect(draw, (px, row4_y + 45, px + tw, row4_y + 75),
                          8, hex_to_rgb(color), None)
        draw.text((px + 12, row4_y + 49), name, fill=hex_to_rgb(BG), font=body_font)
        px += tw + 12

    amods = ["client", "providers", "prompt_cache", "sse", "types", "error"]
    ax = api_x + 480
    for m in amods:
        tw = small_font.getlength(m) + 12
        draw_rounded_rect(draw, (ax, row4_y + 50, ax + tw, row4_y + 70),
                          4, hex_to_rgb(BG), hex_to_rgb(BORDER))
        draw.text((ax + 6, row4_y + 53), m, fill=hex_to_rgb(WHITE), font=small_font)
        ax += tw + 6

    # Telemetry
    tel_x = 840
    tel_w = WIDTH - 40 - tel_x
    draw_rounded_rect(draw, (tel_x, row4_y, tel_x + tel_w, row4_y + 110),
                      12, hex_to_rgb(CARD_BG), hex_to_rgb(GRAY))
    draw.text((tel_x + 20, row4_y + 12), "colotcook-telemetry",
              fill=hex_to_rgb(GRAY), font=header_font)
    draw.text((tel_x + 20, row4_y + 40), "Analytics", fill=hex_to_rgb(WHITE), font=small_font)
    draw.text((tel_x + 20, row4_y + 58), "& Tracing", fill=hex_to_rgb(WHITE), font=small_font)

    # === ARROWS (dependency flow) ===
    arrow_color = hex_to_rgb(BORDER)
    # CLI -> Commands, Tools
    draw.line([(WIDTH // 2, 190), (WIDTH // 2, 210)], fill=arrow_color, width=2)
    # Commands, Tools -> Runtime
    draw.line([(200, 340), (200, 360)], fill=arrow_color, width=2)
    draw.line([(600, 340), (600, 360)], fill=arrow_color, width=2)
    # Runtime -> API
    draw.line([(400, 490), (400, 510)], fill=arrow_color, width=2)
    # Runtime -> Plugins
    draw.line([(900, 490), (900, 510)], fill=arrow_color, width=2)

    # === Footer ===
    draw.text((WIDTH // 2, HEIGHT - 25),
              "unsafe_code = \"forbid\" · clippy::pedantic · 90% coverage · MIT License",
              fill=hex_to_rgb(GRAY), font=small_font, anchor="mt")

    out = os.path.join(os.path.dirname(os.path.dirname(__file__)), "assets", "architecture.png")
    img.save(out, "PNG", optimize=True)
    print(f"Saved: {out}")

if __name__ == "__main__":
    main()
