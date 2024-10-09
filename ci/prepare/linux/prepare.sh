#!/bin/bash
set -euox pipefail

COMPILER=${COMPILER:-gcc}
echo "Compiler: '${COMPILER}'"

# Function to install libbacktrace
# install_libbacktrace() {
#     echo "Installing libbacktrace..."
#     git clone https://github.com/ianlancetaylor/libbacktrace.git
#     cd libbacktrace
#     mkdir build
#     cd build
#     ../configure
#     make
#     sudo make install
#     cd ../..
#     rm -rf libbacktrace
#     echo "libbacktrace installation completed."
# }

# Common dependencies needed for building & testing
DEBIAN_FRONTEND=noninteractive apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -yqq \
build-essential \
g++ \
curl \
wget \
python3 \
zlib1g-dev \
cmake \
git \
qtbase5-dev \
qtchooser \
qt5-qmake \
qtbase5-dev-tools \
valgrind \
xorg xvfb xauth xfonts-100dpi xfonts-75dpi xfonts-scalable xfonts-cyrillic

# Install libbacktrace
# install_libbacktrace

# Compiler specific setup
$(dirname "$BASH_SOURCE")/prepare-${COMPILER}.sh