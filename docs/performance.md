# Terrain Studio render performance

The comparison used the local amd64 release builds of Terrain Studio 0.1.0 and 0.2.0 on an Intel Core i5-10300H with eight logical CPUs. Each command rendered a 960 × 720 PNG with four samples per pixel. Hyperfine ran one warmup and five measured runs per command. The times include process startup, scene loading, rendering, and PNG encoding.

| Scene | 0.1.0 mean | 0.2.0 mean | Reduction in render command time |
| --- | ---: | ---: | ---: |
| Alpine Lake | 1.487 s | 0.890 s | 40% |
| BAS_MONT.TIF imported from the original ISO | 1.927 s | 1.038 s | 46% |
| Checker Study | 0.498 s | 0.431 s | 13% |

The decoded PNGs matched pixel for pixel in all three cases. The BAS_MONT baseline was the noisiest run, ranging from 1.697 to 2.252 seconds.

Version 0.2 prepares terrain-cell height bounds, triangle equations, vertex gradients, and linear material colors once per render. Short row strips distribute expensive regions across available CPU workers. The primary ray search can stop near an opaque water surface.

The original Bryce executable reached its registration screen under the installed Bottles runner. The measured comparison is between the two Terrain Studio releases.
