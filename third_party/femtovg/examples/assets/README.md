# Font fixtures

These fonts are committed for the vendored renderer's unit tests. Tests do not download them at runtime.

| Local file | Upstream revision | SHA-256 | License |
| --- | --- | --- | --- |
| `RobotoFlex-VariableFont.ttf` | Roboto Flex 3.200 | `512f759e24d81543f5629b8edc99c566140a8d161fc2277ac82e64add2f26cf9` | `OFL-RobotoFlex.txt` |
| `amiri-regular.ttf` | Amiri 000.104 | `2ae2ce034ee6d96676f7351bec465f1f86a22e3c4bf2e3bfdb5a653db3bdb46e` | `OFL-Amiri.txt` |

Sources:

- [Roboto Flex font](https://raw.githubusercontent.com/femtovg/femtovg/1bca261ceb0731a3c167c4f29bfb5dc41bf4ff3e/examples/assets/RobotoFlex-VariableFont.ttf).
- [Amiri font](https://raw.githubusercontent.com/femtovg/femtovg/1bca261ceb0731a3c167c4f29bfb5dc41bf4ff3e/examples/assets/amiri-regular.ttf).

These are femtovg's original fixtures, pinned to commit `1bca261ceb0731a3c167c4f29bfb5dc41bf4ff3e`. The downloaded font bytes and filenames are unmodified. Both fonts use the SIL Open Font License 1.1. The bundled notices preserve their embedded copyright and license metadata, including Amiri's reserved font name. They exercise variable-font metrics, glyph layout, and Arabic shaping; they are test fixtures, not application UI fonts.
