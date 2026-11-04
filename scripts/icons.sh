#!/usr/bin/env bash
# Renders the icons from their sources in assets/icons, which need rsvg-convert
# (brew install librsvg). Only run it after changing a source: what it writes is
# committed, so building the app does not need it.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v rsvg-convert >/dev/null || {
	echo "rsvg-convert is missing: brew install librsvg" >&2
	exit 1
}

rsvg-convert -w 1024 -h 1024 assets/icons/app.svg -o resources/icon.png
rsvg-convert -w 1024 -h 1024 assets/icons/app-dev.svg -o resources/icon-dev.png
# 18 points high, for screens of scale 2.
rsvg-convert -w 36 -h 36 assets/icons/menubar.svg -o assets/icons/menubar.png
# The icon the welcome guide shows, and what follows the pointer when it is
# dragged, whose pixels are points.
rsvg-convert -w 256 -h 256 assets/icons/app.svg -o assets/icons/guide.png
rsvg-convert -w 72 -h 72 assets/icons/app.svg -o assets/icons/drag.png
echo "wrote resources/icon.png, resources/icon-dev.png, assets/icons/{menubar,guide,drag}.png"
