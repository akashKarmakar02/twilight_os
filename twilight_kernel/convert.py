
import struct

# Load PSF file
with open("font.psf", "rb") as f:
    # Skip PSF header (first 4 bytes)
    f.seek(4)

    # Read the font data
    font_data = f.read()

# PSF fonts have 256 characters, each 16 bytes (for 16x8 fonts)
CHAR_HEIGHT = 16
CHAR_WIDTH = 8
TOTAL_CHARS = 256  # PSF usually has 256 glyphs
ASCII_PRINTABLE_START = 0x20  # Space ' '
ASCII_PRINTABLE_END = 0x7F    # Delete (not included)

# Extract only printable ASCII characters (0x20 - 0x7E)
psf_fonts = []

for i in range(ASCII_PRINTABLE_START, ASCII_PRINTABLE_END):
    char_bitmap = []
    for row in range(CHAR_HEIGHT):
        byte = font_data[i * CHAR_HEIGHT + row]  # Read the row (8-bit)
        char_bitmap.append(byte)
    
    psf_fonts.append(char_bitmap)

# Generate Rust code
print("pub static PSF_FONTS: [[u8; 16]; 128] = [")
for char in psf_fonts:
    print("    [", ", ".join(f"0b{b:08b}" for b in char), "],")
print("];")

