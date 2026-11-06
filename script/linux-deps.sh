#!/usr/bin/env bash
# System libraries GPUI needs to build and run on Debian/Ubuntu.
set -euo pipefail

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    build-essential clang cmake pkg-config \
    libfontconfig-dev libfreetype-dev \
    libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libx11-xcb-dev libxcb1-dev libxcb-randr0-dev \
    libvulkan-dev mesa-vulkan-drivers \
    libssl-dev libzstd-dev libasound2-dev
