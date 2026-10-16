femtovg 0.27.0 with changes in src/text.rs, src/lib.rs and the OpenGL fragment shader (gradient alpha is dithered too, so translucent gradients over the window backdrop do not band): glyphs sit on whole pixels, each text run is placed on whole device pixels (moving text never resamples between pixels), glyph size is rounded to whole pixels, and glyph coverage gets a contrast curve (sharpen). Text renders crisper.

i-slint-renderer-femtovg 1.18.1 with one change in itemrenderer.rs: a clip with rounded corners draws its layer texture on whole device pixels, so text inside rounded cards is as sharp as text outside them.

Further renderer changes: raster images get mipmaps when shown smaller than their source (no aliasing on icons), and rounded clips use the same squircle outline as the UI shapes. The femtovg copy fills shapes from a centroid fan inset slightly less than the fringe, so the fill overlaps the anti-aliasing edge and pixel-aligned edges no longer show gaps.

femtovg anti-aliasing: the edge fringe is one device pixel wide at every scale. Slint passes a rounded-up scale to the canvas and the stock code divided the fringe by it, so shapes at 125 to 200 percent had a half-pixel edge and looked jagged.

Nine-slice images (the shadows) are drawn without edge anti-aliasing. Each slice was drawn as its own anti-aliased rectangle, so the soft edges of neighbouring slices overlapped and left faint dark hairlines inside every card.

Licences: femtovg is MIT or Apache-2.0 (LICENSE-MIT, LICENSE-APACHE in its folder). i-slint-renderer-femtovg is GPL-3.0-only or the Slint royalty-free or commercial licences (LICENSES in its folder); Wisp uses it under GPL-3.0-only.
