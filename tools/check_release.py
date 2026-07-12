#!/usr/bin/env python3
"""Validate and summarize a Trunk release directory.

The checker deliberately uses only the Python standard library so it can run in
minimal CI environments.
"""

import argparse
import json
import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

MIB = 1024 * 1024
MAX_WASM_BYTES = 25 * MIB

AUDIO_SUFFIXES = {
    ".aac",
    ".flac",
    ".m4a",
    ".mp3",
    ".oga",
    ".ogg",
    ".opus",
    ".wav",
    ".weba",
}
ENVIRONMENT_MAP_SUFFIXES = {".basis", ".dds", ".exr", ".hdr", ".ktx", ".ktx2"}
ENVIRONMENT_MAP_PARTS = {
    "cube_map",
    "cube-map",
    "cubemap",
    "env_map",
    "env-map",
    "envmap",
    "environment",
    "environments",
    "environment_maps",
    "environment-maps",
    "hdri",
    "map",
    "maps",
    "panorama",
    "skybox",
}
TEXT_SUFFIXES = {".css", ".cjs", ".htm", ".html", ".js", ".json", ".map", ".mjs", ".txt", ".webmanifest"}
URL_ATTRIBUTES = {"action", "data", "formaction", "href", "manifest", "poster", "src"}
CSS_URL_RE = re.compile(r"url\(\s*(['\"]?)(.*?)\1\s*\)", re.IGNORECASE | re.DOTALL)
WINDOWS_ABSOLUTE_RE = re.compile(r"^[A-Za-z]:[\\/]")
DEV_MARKERS = (
    (
        "trunk_dev_reload",
        re.compile(r"/_trunk/ws|__trunk_(?:address|ws_protocol)__", re.IGNORECASE),
    ),
    ("websocket", re.compile(r"\bwebsocket\b|\bwss?://", re.IGNORECASE)),
    ("live_reload", re.compile(r"\blive(?:[-_ ]?reload)\b|\blivereload\b", re.IGNORECASE)),
)


class IndexURLParser(HTMLParser):
    """Collect URL-bearing attributes from an HTML document."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.urls = []

    def handle_starttag(self, tag, attrs):
        self._collect(tag, attrs)

    def handle_startendtag(self, tag, attrs):
        self._collect(tag, attrs)

    def _collect(self, tag, attrs):
        attr_map = {str(name).lower(): value for name, value in attrs if name}
        for name, value in attrs:
            name = str(name).lower()
            if not value:
                continue
            if name in URL_ATTRIBUTES:
                self.urls.append(("{}[{}]".format(tag, name), value))
            elif name == "srcset":
                # A normal srcset candidate is "URL [width/density]". Data URLs
                # are external/embedded and are skipped by validate_asset_url.
                for candidate in value.split(","):
                    candidate = candidate.strip()
                    if candidate:
                        self.urls.append(("{}[srcset]".format(tag), candidate.split()[0]))
            elif name == "style":
                for match in CSS_URL_RE.finditer(value):
                    self.urls.append(("{}[style]".format(tag), match.group(2).strip()))

        if tag.lower() == "meta" and attr_map.get("http-equiv", "").lower() == "refresh":
            content = attr_map.get("content") or ""
            match = re.search(r"\burl\s*=\s*(['\"]?)(.*?)\1\s*$", content, re.IGNORECASE)
            if match:
                self.urls.append(("meta[content]", match.group(2).strip()))


def error(code, message, **details):
    item = {"code": code, "message": message}
    item.update(details)
    return item


def mib_value(byte_count):
    return round(byte_count / MIB, 3)


def category_for(relative_path):
    suffix = relative_path.suffix.lower()
    lowered_parts = {part.lower() for part in relative_path.parts}
    lowered_stem = relative_path.stem.lower()

    if suffix == ".wasm":
        return "wasm"
    if suffix in {".js", ".mjs", ".cjs"}:
        return "js"
    if suffix in AUDIO_SUFFIXES:
        return "audio"
    if (
        suffix in ENVIRONMENT_MAP_SUFFIXES
        or lowered_parts.intersection(ENVIRONMENT_MAP_PARTS)
        or any(marker in lowered_stem for marker in ("envmap", "env_map", "skybox", "cubemap"))
    ):
        return "environment_maps"
    return "other"


def inventory(dist, errors):
    files = []
    try:
        candidates = sorted(dist.rglob("*"), key=lambda path: path.as_posix())
    except OSError as exc:
        errors.append(error("dist_unreadable", "Could not enumerate the dist directory", detail=str(exc)))
        return files

    for path in candidates:
        try:
            if not path.is_file():
                continue
            relative = path.relative_to(dist)
            files.append((path, relative, path.stat().st_size))
        except OSError as exc:
            errors.append(
                error(
                    "asset_unreadable",
                    "Could not inspect a release asset",
                    path=str(path),
                    detail=str(exc),
                )
            )
    return files


def decode_local_path(url):
    """Return a decoded local URL path, or None for external/embedded URLs."""
    value = url.strip()
    if not value or value.startswith("#") or value.startswith("?"):
        return None
    if WINDOWS_ABSOLUTE_RE.match(value):
        return value

    try:
        parsed = urlsplit(value)
    except ValueError:
        return value

    # Schemed and protocol-relative URLs are not local build artifacts.
    if parsed.scheme or parsed.netloc:
        return None
    return unquote(parsed.path)


def validate_asset_urls(index_text, errors):
    parser = IndexURLParser()
    try:
        parser.feed(index_text)
        parser.close()
    except Exception as exc:  # HTMLParser extensions can raise on malformed declarations.
        errors.append(error("index_parse_error", "Could not parse index.html", detail=str(exc)))
        return

    # Include URLs in style blocks in addition to style attributes.
    for match in CSS_URL_RE.finditer(index_text):
        parser.urls.append(("style[url]", match.group(2).strip()))

    seen = set()
    for context, url in parser.urls:
        key = (context, url)
        if key in seen:
            continue
        seen.add(key)
        local_path = decode_local_path(url)
        if local_path is None:
            continue

        normalized = local_path.replace("\\", "/")
        path_parts = [unquote(part) for part in normalized.split("/")]
        reason = None
        if WINDOWS_ABSOLUTE_RE.match(local_path):
            reason = "Windows-absolute path"
        elif local_path.startswith(("/", "\\")):
            reason = "root-absolute path"
        elif "\\" in local_path:
            reason = "backslash path"
        elif ".." in path_parts:
            reason = "parent-directory traversal"

        if reason:
            errors.append(
                error(
                    "unsafe_asset_url",
                    "index.html contains an asset URL that is not subpath-safe",
                    context=context,
                    url=url,
                    reason=reason,
                )
            )


def check_dev_markers(files, errors):
    for path, relative, _size in files:
        if relative.suffix.lower() not in TEXT_SUFFIXES:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            errors.append(
                error(
                    "asset_unreadable",
                    "Could not scan an emitted text asset",
                    path=relative.as_posix(),
                    detail=str(exc),
                )
            )
            continue

        found = [name for name, pattern in DEV_MARKERS if pattern.search(text)]
        if found:
            errors.append(
                error(
                    "development_reload_marker",
                    "Development reload/WebSocket code was found in release output",
                    path=relative.as_posix(),
                    markers=found,
                )
            )


def build_size_report(files):
    categories = ("wasm", "js", "audio", "environment_maps", "other")
    totals = {name: {"bytes": 0, "files": 0} for name in categories}

    for _path, relative, size in files:
        category = category_for(relative)
        totals[category]["bytes"] += size
        totals[category]["files"] += 1

    total_bytes = sum(item["bytes"] for item in totals.values())
    report = {}
    for name in categories:
        report[name] = {
            "bytes": totals[name]["bytes"],
            "mib": mib_value(totals[name]["bytes"]),
            "files": totals[name]["files"],
        }
    report["total"] = {
        "bytes": total_bytes,
        "mib": mib_value(total_bytes),
        "files": len(files),
    }
    return report


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description="Validate and size a Trunk release build")
    parser.add_argument(
        "--dist",
        default="dist",
        metavar="PATH",
        help="release output directory to inspect (default: dist)",
    )
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    dist = Path(args.dist)
    errors = []

    if not dist.exists():
        errors.append(error("dist_missing", "Release directory does not exist", path=str(dist)))
        files = []
    elif not dist.is_dir():
        errors.append(error("dist_not_directory", "Release path is not a directory", path=str(dist)))
        files = []
    else:
        files = inventory(dist, errors)

    index_path = dist / "index.html"
    if not index_path.is_file():
        errors.append(error("index_missing", "Release output is missing index.html", path=str(index_path)))
    else:
        try:
            index_text = index_path.read_text(encoding="utf-8-sig")
        except (OSError, UnicodeError) as exc:
            errors.append(error("index_unreadable", "Could not read index.html as UTF-8", detail=str(exc)))
        else:
            validate_asset_urls(index_text, errors)

    wasm_files = [(relative, size) for _path, relative, size in files if relative.suffix.lower() == ".wasm"]
    if not wasm_files:
        errors.append(error("wasm_missing", "Release output contains no WebAssembly file"))
    elif len(wasm_files) != 1:
        errors.append(
            error(
                "wasm_count",
                "Release output must contain exactly one unambiguous WebAssembly file",
                count=len(wasm_files),
                paths=[relative.as_posix() for relative, _size in wasm_files],
            )
        )

    for relative, size in wasm_files:
        if size > MAX_WASM_BYTES:
            errors.append(
                error(
                    "wasm_too_large",
                    "WebAssembly output exceeds the 25 MiB release limit",
                    path=relative.as_posix(),
                    bytes=size,
                    mib=mib_value(size),
                    limit_bytes=MAX_WASM_BYTES,
                )
            )

    check_dev_markers(files, errors)
    sizes = build_size_report(files)
    report = {
        "ok": not errors,
        "dist": str(dist),
        "limits": {"wasm_bytes": MAX_WASM_BYTES, "wasm_mib": 25.0},
        "sizes": sizes,
        "wasm_outputs": [
            {"path": relative.as_posix(), "bytes": size, "mib": mib_value(size)}
            for relative, size in wasm_files
        ],
        "errors": errors,
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
