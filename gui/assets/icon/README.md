# The application icon

The Cover Flow, abstracted: the playing album lit up front, its neighbours receding either side, all
of it on the reflective black floor under the backdrop glow - the *Player* view reduced to a mark.

| file | what it is |
|---|---|
| `phonoscule.svg` | the artwork. Source of truth, and what a Linux icon theme can use directly |
| `phonoscule-small.svg` | the same mark for 16-32px, where the full one turns to mud: only the playing cover, larger |
| `phonoscule.ico` | generated. 16/24/32/48/64/128/256, embedded in the `.exe` by `../../build.rs` |
| `phonoscule-256.png` | generated. The window icon the running player sets, and a raster for Linux |
| `pack-ico.py` | packs PNGs into a `.ico`. Standard library only |

The two generated files are committed, so an ordinary build - and `cargo install` - needs no image
tooling and no browser. Regenerate them only when the artwork changes.

## Regenerating

Render each size from the vector at that size rather than downsampling one large bitmap, so the small
ones stay crisp. Sizes at or below 32 come from `phonoscule-small.svg`, the rest from
`phonoscule.svg`.

With any SVG renderer - `rsvg-convert -w 48 -h 48`, `inkscape -w 48 -h 48`, `magick -background none
-density ... ` - produce `16.png` through `256.png`, then:

```sh
python pack-ico.py phonoscule.ico 256.png 128.png 64.png 48.png 32.png 24.png 16.png
cp 256.png phonoscule-256.png
```

On a Windows machine with none of those installed, headless Edge or Chrome will do it. Two things to
know, both learned the hard way:

- Give the SVG an intrinsic size matching the window. A standalone SVG whose size differs gets
  scaled and positioned by rules of Chromium's own, and the screenshot comes out cropped and offset.
- Keep it a standalone SVG. Chromium honours `--default-background-color=00000000`, and so preserves
  transparency, for an SVG document but not for one inlined into an HTML page, where it writes opaque
  RGB and the rounded corners come out filled.

```sh
size=48
sed "s|viewBox=\"0 0 256 256\"|viewBox=\"0 0 256 256\" width=\"$size\" height=\"$size\"|" \
    phonoscule.svg > sized.svg
msedge --headless --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
       --default-background-color=00000000 --window-size=$size,$size \
       --screenshot=$size.png sized.svg
```

Whatever renders them, check the results before packing: every PNG should be RGBA, its corner pixel
clear, and the cover's warm gradient reaching near-full red. A crop or a lost alpha channel makes a
perfectly valid `.ico` that looks wrong only once it is on a taskbar.

## Editing the artwork

Note that an XML comment may not contain a double hyphen, so the prose in these two files uses single
dashes where the rest of the tree uses `--`.
