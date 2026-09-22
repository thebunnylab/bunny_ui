# The Linux shell's proving ground: one Debian image with the libraries
# the shell links, the two displays it can open (a headless Weston for
# the Wayland door, an Xvfb for the X11 door), Mesa's software GL and
# Vulkan (llvmpipe and lavapipe), the fonts the text tests expect, and
# the WPE WebKit stack the webview loads at run time. Rust is installed
# once, in the image; the cargo registry and the target directory live
# in named volumes so a rebuild of the image keeps them.
#
# Built and run by `run.sh` in this directory.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive \
    PATH=/root/.cargo/bin:$PATH \
    RUSTUP_HOME=/root/.rustup \
    CARGO_HOME=/root/.cargo

# the link-time libraries of the shell (thirteen sonames), the run-time
# ones it dlopens (EGL, wayland-egl, vulkan), the displays, the fonts,
# and the WPE WebKit stack
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl build-essential pkg-config git procps \
    libxcb1-dev libxcb-xfixes0-dev libxcb-xkb-dev libxcb-shm0-dev \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
    libdbus-1-dev libsecret-1-dev libfontconfig-dev libfreetype-dev libharfbuzz-dev \
    libegl1 libgles2 libgl1-mesa-dri libegl-mesa0 libwayland-egl1 \
    mesa-vulkan-drivers vulkan-tools \
    wayland-protocols wayland-utils weston xvfb xauth x11-utils dbus dbus-daemon \
    fonts-dejavu fonts-noto-color-emoji hicolor-icon-theme adwaita-icon-theme \
    python3 \
    libwpewebkit-2.0-1 libwpewebkit-2.0-dev libwpebackend-fdo-1.0-dev libwpe-1.0-dev \
    gstreamer1.0-plugins-base \
    && rm -rf /var/lib/apt/lists/*

# stable Rust, the minimal profile
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
    && rustc --version && cargo --version

WORKDIR /work
