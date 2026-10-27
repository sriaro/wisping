# Renderer tests

The 14 integration test sources and the three examples in `../book/` were restored unchanged from the [femtovg 0.27.0 release archive](https://static.crates.io/crates/femtovg/femtovg-0.27.0.crate). That archive's manifest matches this vendored manifest and identifies upstream commit `0eb55780894a9200e9a70ca3d16bd1c641281a44`.

Archive SHA-256: `31eebf3ab1b76359ccd5f850c07a7c6b2722c64d9127c40e02358e83f3ff9b1a`.

Run from the repository root:

```sh
cargo test --manifest-path third_party/femtovg/Cargo.toml --release --all-features -- --test-threads=1 --nocapture
cargo test --manifest-path third_party/femtovg/Cargo.toml --release --all-features security_wgpu_many_contour_gradient_has_no_validation_error -- --ignored --test-threads=1 --nocapture
```

GPU integration tests print a skip message if no adapter is available. The second command explicitly runs the native WGPU security regression, which requires an adapter and fails if none is available. Font provenance and licenses are in `../examples/assets/README.md`.
