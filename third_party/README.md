femtovg 0.27.0 with changes in src/text.rs, src/lib.rs, src/gradient_store.rs and the OpenGL fragment shader (gradient alpha is dithered too, so translucent gradients over the window backdrop do not band): glyphs sit on whole pixels, each text run is placed on whole device pixels (moving text never resamples between pixels), glyph size is rounded to whole pixels, and glyph coverage gets a contrast curve (sharpen). Text renders crisper.

i-slint-renderer-femtovg 1.18.1 with one change in itemrenderer.rs: a clip with rounded corners draws its layer texture on whole device pixels, so text inside rounded cards is as sharp as text outside them.

Further renderer changes: raster images get mipmaps when shown smaller than their source (no aliasing on icons), and rounded clips use the same squircle outline as the UI shapes. The femtovg copy fills shapes from a centroid fan inset slightly less than the fringe, so the fill overlaps the anti-aliasing edge and pixel-aligned edges no longer show gaps.

femtovg anti-aliasing: the edge fringe is one device pixel wide at every scale. Slint passes a rounded-up scale to the canvas and the stock code divided the fringe by it, so shapes at 125 to 200 percent had a half-pixel edge and looked jagged.

An image with a solid `colorize` is drawn with femtovg's image tint (a channel-wise multiplication) instead of baking a coloured copy into a fresh texture every time the colour changes. Every colorized image in Wisping (the icon SVGs and the shadow PNGs) is white, so the result equals Slint's alpha-mask colorize, and an animated colour no longer allocates and renders one texture per image per frame. A gradient `colorize` still takes the stock offscreen path.

femtovg gradient textures: a multi-stop gradient keeps its 256 by 1 texture until two flushes pass without it, and released textures (up to 16) are refilled for new gradients instead of being deleted. Slint's renderer flushes twice per frame when a rendering notifier is set, so the stock store deleted and recreated every gradient texture on every frame; after a language switch that texture creation grew to about 15 ms per frame. An animated gradient now rewrites a spare texture instead of creating one.

Nine-slice images (the shadows) are drawn without edge anti-aliasing. Each slice was drawn as its own anti-aliased rectangle, so the soft edges of neighbouring slices overlapped and left faint dark hairlines inside every card.

Licences: femtovg is MIT or Apache-2.0 (LICENSE-MIT, LICENSE-APACHE in its folder). i-slint-renderer-femtovg is GPL-3.0-only or the Slint royalty-free or commercial licences (LICENSES in its folder); Wisping uses it under GPL-3.0-only.

Both copies carry only what the build needs. femtovg's book, examples, tests, lockfile and the registry packaging files (Cargo.toml.orig) are left out, and both manifests keep only the library target and its dependencies.

Failed gradient uploads release their image allocation, including textures taken from the reuse pool.

Vendored source comments and documentation comments are stripped; copyright, licence and source attribution notices remain. The femtovg documentation-only missing-docs lint is removed.

Test modules, fixtures, mock renderers and test harnesses are removed from both vendored copies, as are debug assertions and femtovg's OpenGL error checks, which only ran in debug builds; femtovg no longer depends on `log`.

Code for features Wisping does not enable is removed as well: the wgpu renderers and their shader, femtovg's text layout, image loading, serde and debug inspector code, and the WebAssembly paths of both copies. femtovg's public API is cut down to what the renderer calls: the unused gradient constructors and paint setters, colour-matrix image filters, canvas shadows and skew, external textures, surfaceless rendering, path arcs and dashing, font loading from files and copied byte buffers, font metrics and text style settings are removed. The fragment shader variants left without a caller (the colour-matrix filter and the unclipped colour fill) are no longer compiled, and the native texture lookup and the error kinds nothing produces are removed. Importing an existing OpenGL texture as an image, which Wisping never does, is removed from both copies, and their manifests drop the dependencies the remaining code does not use. The manifests list only the features the build uses (femtovg `swash`, i-slint-renderer-femtovg `default` and `opengl`).
