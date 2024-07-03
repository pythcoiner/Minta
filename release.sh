#!/bin/sh

# Variables
REPO="pythcoiner/modbus485_debugger"
TAG="v0.1.1"
NAME="modbus485_debugger"
RELEASE_TITLE="Modbus485 Debugger $TAG"
CHANGELOG_PATH="changelog.txt"
RELEASE_NOTES=$(cat "$CHANGELOG_PATH")
BUILD_DIR="build"

# Ensure cargo is installed
if ! command -v cargo &> /dev/null
then
    echo "cargo could not be found. Please install Rust and Cargo."
    exit 1
fi

# Clean build directory
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"

# Build for Linux
echo "Building for Linux..."
cargo build --release
cp "target/release/$NAME" "$BUILD_DIR/$NAME-$TAG-x86_64-linux-gnu"

# Build for Windows
echo "Building for Windows..."
cargo build --release --target=x86_64-pc-windows-gnu
cp "target/x86_64-pc-windows-gnu/release/$NAME.exe" "$BUILD_DIR/$NAME-$TAG-x86_64-windows-gnu.exe"

## remove tag
#git tag -d "$TAG"
## Remove from remote repository
#git push --delete origin "$TAG"
#git push --delete github "$TAG"

# Tagging
git tag "$TAG"
git push origin "$TAG"
git push github "$TAG"

# Create a GitHub release
gh release create "$TAG" \
  --repo "$REPO" \
  --title "$RELEASE_TITLE" \
  --notes "$RELEASE_NOTES" \
  "$BUILD_DIR/$NAME-$TAG-x86_64-linux-gnu" \
  "$BUILD_DIR/$NAME-$TAG-x86_64-windows-gnu.exe"

# Create a GitLab release
glab  release create "$TAG" \
    --repo "rust/modbus485_debugger" \
    --name "$RELEASE_TITLE" \
    --notes "$RELEASE_NOTES" \
    "$BUILD_DIR/$NAME-$TAG-x86_64-linux-gnu" \
    "$BUILD_DIR/$NAME-$TAG-x86_64-windows-gnu.exe"

echo "Release $TAG created successfully"