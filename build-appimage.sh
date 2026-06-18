#!/usr/bin/env bash
set -euo pipefail

APP_NAME="llm-translate"
APP_DIR="build/AppDir"
DIST_DIR="dist"
OUTPUT="LLM_Translator-x86_64.AppImage"

echo "==> Cleaning previous build artifacts"
rm -rf "$APP_DIR" "$DIST_DIR" "*.spec"

echo "==> Building executable with PyInstaller"
uv run pyinstaller --onefile --name "$APP_NAME" \
    --add-data "src/local_llm_translator/prompts:local_llm_translator/prompts" \
    --collect-all "pymupdf4llm" \
    --collect-all "pymupdf" \
    --collect-all "onnxruntime" \
    --distpath "$DIST_DIR" \
    --workpath "build/pyinstaller" \
    "src/local_llm_translator/__main__.py"

echo "==> Creating AppDir structure"
mkdir -p "$APP_DIR/usr/bin"
cp "$DIST_DIR/$APP_NAME" "$APP_DIR/usr/bin/"

cat > "$APP_DIR/AppRun" <<'EOF'
#!/bin/bash
SELF_DIR="$(dirname "$(readlink -f "$0")")"
exec "$SELF_DIR/usr/bin/llm-translate" "$@"
EOF
chmod +x "$APP_DIR/AppRun"

cat > "$APP_DIR/$APP_NAME.desktop" <<EOF
[Desktop Entry]
Name=LLM Translator
Exec=$APP_NAME
Terminal=true
Type=Application
Categories=Office;
Icon=$APP_NAME
EOF

# Generate a minimal 1x1 PNG icon (appimagetool requires an icon)
uv run python -c "
import struct, zlib
width, height = 1, 1
raw = b''
for y in range(height):
    raw += b'\\x00' + bytes([0, 120, 215, 255])
def chunk(ctype, data):
    c = ctype + data
    return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
ihdr = struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0)
png = b'\\x89PNG\\r\\n\\x1a\\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
with open('$APP_DIR/$APP_NAME.png', 'wb') as f:
    f.write(png)
"

echo "==> Building AppImage"
ARCH=x86_64 appimagetool "$APP_DIR"

echo "==> Done: $OUTPUT"
ls -lh "$OUTPUT"
