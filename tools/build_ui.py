"""Build autonomous HTML pages from readable templates, CSS and JS sources."""

import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def render_pages(protocol_js=None):
    if protocol_js is None:
        protocol_js = (ROOT / "web/protocol_spec.js").read_text()
    pages = {}
    for name in ("webusb", "ble"):
        template = (ROOT / f"web/{name}.html").read_text()
        page = template.replace("{{STYLE}}", (ROOT / f"web/{name}.css").read_text().rstrip())
        if name == "webusb":
            scripts = [protocol_js] + [
                (ROOT / "web" / filename).read_text()
                for filename in ("protocol_codec.js", "usb_session.js", "usb_app.js")
            ]
            scripts.append("const pagerApp = new PagerApp(document, navigator.usb);\n")
            page = page.replace("{{SCRIPT}}", "\n".join(scripts).rstrip())
        if "{{" in page or "</script" in "\n".join(scripts if name == "webusb" else []):
            raise ValueError("invalid UI template or embedded script")
        pages[f"{name}_client.html"] = page
    return pages


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for name, content in render_pages().items():
        for path in (ROOT / name, ROOT / "dist/ui" / name):
            if args.check:
                if not path.exists() or path.read_text() != content:
                    raise SystemExit(f"stale UI artifact: {path.relative_to(ROOT)}")
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)


if __name__ == "__main__":
    main()
