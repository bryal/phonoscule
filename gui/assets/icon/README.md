# The application icon

The Cover Flow, abstracted: the playing album lit up front, its neighbours receding either side, all
of it on the reflective black floor under the backdrop glow - the *Player* view reduced to a mark.

| file | what it is |
|---|---|
| `phonoscule.svg` | the artwork, and the only copy of it |
| `phonoscule-small.svg` | the same mark for 16-32px, where the full one turns to mud: only the playing cover, larger |

There is nothing generated to commit. [`../../build.rs`](../../build.rs) renders these at build time
and, on Windows, packs them into the `.ico` it compiles into the executable; the running player uses
the 256px render as its window icon on every platform. resvg carries its own rasteriser, so this
asks for no tool on the machine, and editing the artwork is the whole of changing the icon.

## Editing

Sizes at or below 32 come from `phonoscule-small.svg`, the rest from `phonoscule.svg`, and each is
rendered from the vector at its own size rather than downsampled - so check both files after a
change, and look at the result somewhere it will actually be seen. A 16px icon is three pixels of
cover and a suggestion of a glow; detail that reads beautifully at 256 is mud there, which is why
there are two files rather than one.

Keep the artwork square with the same 256-unit `viewBox`, and keep the rounded corners transparent:
these become a taskbar icon, not a tile.

The `<title>` in each file is metadata, not drawn.
