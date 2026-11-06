# The application icon

The Cover Flow: the playing album square on with two beamed eighth notes on its cover, two covers
either side tilted towards it, the nearer pair with an eighth note each, and every cover above its
reflection - the *Player* view reduced to a mark. It has no backdrop, so it stands on whatever the
taskbar or dock is, light or dark.

| file | what it is |
|---|---|
| `phonoscule.svg` | the artwork, and the only copy of it |
| `phonoscule-small.svg` | the same mark for 16-32px, where the full one turns to mud: one cover either side, larger, on whole pixels, and only the middle notes |

There is nothing generated to commit. [`../../build.rs`](../../build.rs) renders these at build time
and, on Windows, packs them into the `.ico` it compiles into the executable; the running player uses
the 256px render as its window icon on every platform. resvg carries its own rasteriser, so this
asks for no tool on the machine, and editing the artwork is the whole of changing the icon.

The Android app's launcher icon is the same `phonoscule.svg`, on a backdrop of its own (see
[`../../../android/app/res/drawable/ic_launcher.xml`](../../../android/app/res/drawable/ic_launcher.xml)).

## Editing

Sizes at or below 32 come from `phonoscule-small.svg`, the rest from `phonoscule.svg`, and each is
rendered from the vector at its own size rather than downsampled - so check both files after a
change, and look at the result somewhere it will actually be seen, on a light background and a dark
one. Detail that reads beautifully at 256 is mud at 16, which is why there are two files rather than
one.

The covers are placed where the player's camera sees them (see `gui/src/coverflow.rs`): a side
cover is tallest at its outer edge, nearest the viewer, and tucks behind the cover in front of it.

Keep the artwork square with the same 256-unit `viewBox`, and the backdrop transparent: these become
a taskbar icon, not a tile.

The `<title>` in each file is metadata, not drawn.
