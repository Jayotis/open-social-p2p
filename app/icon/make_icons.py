"""Makes the Android launcher icons from app-icon.png. Needs Pillow.

Run from the repo root after changing the artwork:

    python3 app/icon/make_icons.py

Writes two kinds of icon into app/android/app/src/main/res:

- ic_launcher.png (48 dp): the whole picture, for Android before 8.
- ic_launcher_foreground.png (108 dp): the picture for an adaptive icon,
  which the launcher crops to its own shape. Only the middle 72 dp shows,
  and only a circle 66 dp across is sure to, so the picture is scaled to
  fit the network inside that circle. The background layer is the
  picture's dark edge colour (values/ic_launcher_background.xml).
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
RES = HERE.parent / "android" / "app" / "src" / "main" / "res"

# Pixels per dp at each density.
DENSITIES = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}

# The picture's width as a share of the 108 dp canvas. The outermost
# network nodes reach 475 px from the centre of the 1254 px picture, and
# must stay inside the 66 dp circle: 1254 / (475 * 108 / 33) = 0.807.
FOREGROUND_SCALE = 0.807


def main():
    art = Image.open(HERE / "app-icon.png").convert("RGBA")
    for name, density in DENSITIES.items():
        folder = RES / f"mipmap-{name}"
        folder.mkdir(exist_ok=True)

        size = round(48 * density)
        art.resize((size, size), Image.LANCZOS).save(folder / "ic_launcher.png", optimize=True)

        canvas = round(108 * density)
        inner = round(canvas * FOREGROUND_SCALE)
        foreground = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
        offset = (canvas - inner) // 2
        foreground.paste(art.resize((inner, inner), Image.LANCZOS), (offset, offset))
        foreground.save(folder / "ic_launcher_foreground.png", optimize=True)


if __name__ == "__main__":
    main()
