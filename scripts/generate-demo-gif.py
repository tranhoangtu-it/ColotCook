#!/usr/bin/env python3
"""Generate a terminal demo GIF for ColotCook README."""

from PIL import Image, ImageDraw, ImageFont
import os

WIDTH = 900
HEIGHT = 560
BG = "#1a1b26"
FG = "#a9b1d6"
GREEN = "#9ece6a"
BLUE = "#7aa2f7"
CYAN = "#7dcfff"
YELLOW = "#e0af68"
RED = "#f7768e"
PURPLE = "#bb9af7"
GRAY = "#565f89"
PROMPT_COLOR = "#73daca"
DIM = "#3b4261"

def hex_to_rgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))

def create_frame(lines, cursor_line=None):
    """Create a single terminal frame."""
    img = Image.new("RGB", (WIDTH, HEIGHT), hex_to_rgb(BG))
    draw = ImageDraw.Draw(img)

    try:
        font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 14)
        bold_font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 14)
        title_font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 12)
    except (OSError, IOError):
        font = ImageFont.load_default()
        bold_font = font
        title_font = font

    # Window chrome
    draw.rounded_rectangle((0, 0, WIDTH, HEIGHT), radius=10,
                           fill=hex_to_rgb(BG), outline=hex_to_rgb(DIM))
    # Title bar
    draw.rectangle((0, 0, WIDTH, 35), fill=hex_to_rgb("#16161e"))
    draw.rounded_rectangle((0, 0, WIDTH, 35), radius=10, fill=hex_to_rgb("#16161e"))
    # Traffic lights
    draw.ellipse((15, 10, 27, 22), fill=hex_to_rgb(RED))
    draw.ellipse((35, 10, 47, 22), fill=hex_to_rgb(YELLOW))
    draw.ellipse((55, 10, 67, 22), fill=hex_to_rgb(GREEN))
    # Title
    draw.text((WIDTH // 2, 11), "colotcook — Terminal", fill=hex_to_rgb(GRAY),
              font=title_font, anchor="mt")

    y = 50
    for i, (text, color) in enumerate(lines):
        draw.text((20, y), text, fill=hex_to_rgb(color), font=font)
        y += 20
        if y > HEIGHT - 20:
            break

    return img

def main():
    # Frame 1: Start
    f1 = create_frame([
        ("$ colotcook \"explain the architecture of this project\"", PROMPT_COLOR),
        ("", FG),
        ("╭──────────────────────────────────────────────────────╮", BLUE),
        ("│  ColotCook v0.1.0 · claude-opus-4-6 · workspace-write │", BLUE),
        ("╰──────────────────────────────────────────────────────╯", BLUE),
        ("", FG),
        ("I'll analyze the project structure for you.", FG),
        ("", FG),
        ("> Reading Cargo.toml...", YELLOW),
        ("> Reading crates/api/src/lib.rs...", YELLOW),
        ("> Reading crates/runtime/src/lib.rs...", YELLOW),
        ("", FG),
        ("This is a Rust workspace with 7 crates:", FG),
        ("", FG),
        ("1. colotcook-api     — Provider abstraction (5 providers)", GREEN),
        ("2. colotcook-cli     — Terminal UI with markdown rendering", GREEN),
        ("3. colotcook-runtime — Agent loop, sessions, sandbox", GREEN),
        ("4. colotcook-tools   — 19 built-in tools", GREEN),
        ("5. colotcook-commands— 15 slash commands", GREEN),
        ("6. colotcook-plugins — Plugin discovery & hooks", GREEN),
        ("7. colotcook-telemetry— Analytics & tracing", GREEN),
    ])

    # Frame 2: Fix bug
    f2 = create_frame([
        ("$ colotcook --model ollama:llama3 \"fix the bug in parser.rs\"", PROMPT_COLOR),
        ("", FG),
        ("╭──────────────────────────────────────────╮", BLUE),
        ("│  ColotCook v0.1.0 · llama3 · read-only   │", BLUE),
        ("╰──────────────────────────────────────────╯", BLUE),
        ("", FG),
        ("> Reading src/parser.rs...", YELLOW),
        ("> Found issue at line 42: off-by-one error", RED),
        ("", FG),
        ("The `next_token()` function uses `<` instead of `<=`", FG),
        ("when checking the buffer boundary.", FG),
        ("", FG),
        ("> Editing src/parser.rs...", YELLOW),
        ("", FG),
        ("Applied fix:", FG),
        ("-    if self.pos < self.input.len() {", RED),
        ("+    if self.pos <= self.input.len() {", GREEN),
        ("", FG),
        ("> Running cargo test...", YELLOW),
        ("  23 passed, 0 failed", GREEN),
        ("", FG),
        ("The fix is applied and all tests pass. ✓", GREEN),
    ])

    # Frame 3: Session management
    f3 = create_frame([
        ("$ colotcook --resume latest /status", PROMPT_COLOR),
        ("", FG),
        ("  Session          ~/.colotcook/sessions/abc123.jsonl", FG),
        ("  Messages         14", FG),
        ("  Model            claude-opus-4-6", CYAN),
        ("  Permission mode  workspace-write", GREEN),
        ("  Token usage      12,847 input · 3,291 output", FG),
        ("  Estimated cost   $0.42", YELLOW),
        ("", FG),
        ("$ colotcook /help", PROMPT_COLOR),
        ("", FG),
        ("  Slash Commands:", BLUE),
        ("  /help          Show available commands", FG),
        ("  /status        Session info, model, usage", FG),
        ("  /compact       Compress conversation history", FG),
        ("  /model         Switch AI model", FG),
        ("  /export        Export conversation to file", FG),
        ("  /config        Inspect merged configuration", FG),
        ("  /plugins       Manage plugins", FG),
        ("  /diff          Show git workspace changes", FG),
        ("  /cost          Show token usage and cost", FG),
        ("  /version       Print version info", FG),
    ])

    # Frame 4: Multi-provider
    f4 = create_frame([
        ("$ colotcook --model gemini-2.5-pro \"review this PR\"", PROMPT_COLOR),
        ("╭────────────────────────────────────────────╮", BLUE),
        ("│  ColotCook v0.1.0 · gemini-2.5-pro · prompt │", BLUE),
        ("╰────────────────────────────────────────────╯", BLUE),
        ("", FG),
        ("> Reading git diff...", YELLOW),
        ("> Analyzing 12 changed files...", YELLOW),
        ("", FG),
        ("PR Review Summary:", PURPLE),
        ("", FG),
        ("✓ Code quality: Clean, follows conventions", GREEN),
        ("✓ Tests: All 47 tests pass", GREEN),
        ("⚠ Performance: Consider caching in line 234", YELLOW),
        ("✗ Security: SQL query at line 89 needs parameterization", RED),
        ("", FG),
        ("Overall: Approve with minor changes", GREEN),
        ("", FG),
        ("───────────────────────────────────", DIM),
        ("Providers: Anthropic · OpenAI · Gemini · Grok · Ollama", GRAY),
        ("Tools: 19 built-in · MCP protocol · Plugin system", GRAY),
    ])

    # Save as GIF
    frames = [f1, f2, f3, f4]
    out = os.path.join(os.path.dirname(os.path.dirname(__file__)), "assets", "demo.gif")
    frames[0].save(
        out,
        save_all=True,
        append_images=frames[1:],
        duration=4000,  # 4 seconds per frame
        loop=0,
        optimize=True,
    )
    print(f"Saved: {out}")

    # Also save individual frames as PNG for README
    for i, f in enumerate(frames):
        frame_path = os.path.join(os.path.dirname(os.path.dirname(__file__)),
                                  "assets", f"demo-frame-{i+1}.png")
        f.save(frame_path, "PNG", optimize=True)
        print(f"Saved: {frame_path}")

if __name__ == "__main__":
    main()
