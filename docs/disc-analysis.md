# Bryce 2 disc analysis

The supplied disc is a 674,492,416-byte ISO 9660 image labeled `BRYCE2`, mastered on 16 December 1996. Its SHA-256 is `11a3595e19245d7e73bbbf0ca400a7414b80c3c6d1d5c736ac5d2dbc235b0763`.

The disc root contains a Windows InstallShield 3 archive named `DATA.1`. The extracted `FILTER/Bryce2.exe` is a PE32 x86 GUI executable with SHA-256 `ef0b5e2e2b71ee262d430aba8c1abe6a91b458b6d9d792aeda103ade03f334d8`. Its adjacent installer entries identify separate terrain, atmosphere, materials, scene and renderer controls. The disc's `SAMPLE` folder contains original renders and `.BRC` scene files. The scene samples opened for inspection use an Axiom composite container identified by `CCmF`. The terrain folder contains TIFF images intended for grayscale elevation maps.

## REA inspection

I extracted the executable locally with the InstallShield 3 unpacker idecomp and examined it statically using REA 6.10.1, Ghidra 12.2 and JDK 25. [REA repository](https://github.com/morluto/rea) · [InstallShield 3 unpacker](https://github.com/lephilousophe/idecomp)

```sh
7z x /path/to/BRYCE2.iso DATA.1
python3 /path/to/idecomp.py DATA.1 '*/Bryce2.exe'
GHIDRA_INSTALL_DIR=/path/to/ghidra rea function \
  "$PWD/FILTER/Bryce2.exe" ram:0x0041e0b0 --provider ghidra --json
```

REA resolved the routine at `0x0041e0b0` to a small record initializer shared by multiple callers. One caller references the original pyramid resource-loading label and tags. This supports the separation of primitive records from preset resources, while the disc and the supplied visual references establish the visible authoring workflow. The Rust scene data and renderer were written independently. The editor implements its own geometry, procedural materials, lighting, and reflection algorithms.

## Disc compatibility

Terrain Studio reads the ISO 9660 directory directly with no mount step and imports every one of the 58 unique TIFF names in the disc's `TERRAIN` directory. The package contains the Rust implementation and original examples are generated on demand. The original composite scene format is described below.

## Composite terrain import in 0.2.0

The 0.1 renderer and material system were independent implementations based on the disc and visual references. The REA inspection above covered one initializer. Version 0.2 adds an importer grounded in two more native functions and tested against the original scene bytes.

| Original function | REA/Ghidra finding |
| --- | --- |
| `0x0056a430` | Writes a 72-byte header at file offset `0x248`. The x86 routine byte-swaps the numeric fields before writing. |
| `0x0056b2e0` | Seeks to the offset stored at resource descriptor offset `+8`, reads a 12-byte big-endian block header, copies the used bytes, follows the next offset, and checks the total against descriptor field `+4`. |

The REA evidence IDs are `ev_5202d84b706ce9f57c99687b0aa2ed689a794926cbfd2a15e743e8a22342a395` and `ev_57e56adf330c7a0ff2935e86b4dab840b2549afcc1a4ec51c14ce8a130e592f9`, respectively. Substitute the listed address in the command above to reproduce the inspection.

The samples establish the file-layout interpretation. Header fields at `0x24c` and `0x250` give the resource table length and first block offset. Each 12-byte table entry has a length at `+4` and first block offset at `+8`. Each linked block stores allocated bytes, used payload bytes, and next block offset at `+0`, `+4`, and `+8`. Offsets are from the start of the file.

A terrain record contains its square resolution at `0x348` followed by a big-endian unsigned 16-bit elevation grid at `0x350`. The importer requires the record's dimensions, internal extent, and exact payload size to agree. Across 74 sample scenes it found 308 terrain records in 64 scenes. For example, `FIORD.BRC` resource 66 is 512 × 512 and `ABISKO.BRC` resource 94 is 512 × 512. Resource numbers refer to the original table indices.

The original executable reached first-run registration using the local Bottles Wine runner. Further investigation proceeded statically. Performance measurements cover the Rust renderer only.

## Scene import in 0.3.0

The scene importer follows the class directory through composite resources 16 and 17. Class entries are 32 bytes wide and hold their member resource indices at `+16` and `+20`. Member rows are 60 bytes wide and identify the main and auxiliary object records at `+4` and `+8`. The count is stored at `+12` in the corresponding directory header. This relationship was verified across the disc's sample scenes.

| Original function | REA/Ghidra finding | Evidence ID |
| --- | --- | --- |
| `0x00595340` | Allocates the `0x374`-byte perspective camera record. | `ev_32b12cd194143329ec81676cad0fe8e0a48ba26fed0140a5d314297e85e85ad5` |
| `0x005941b0` | Swaps camera forward at `0x2ec`, up at `0x304`, focal target at `0x31c`, and angle at `0x33c`. | `ev_b2646efe96df097c428a2497c7539898eb1082249fc32fb665ac29a8d2d908b6` |
| `0x00595580` | Forms the view span with `tan(stored_angle × 0.8 ÷ 2)` and applies the saved lens corrections. | `ev_bf7314b7e392ab4726cbc37156d5e5ecf8357b59081252fbead869b7c0ec92a6` |
| `0x0051f690` | Resizes the terrain record to `0x350 + 2 × side²` bytes. | `ev_feaa39254bc019c3074b6affe19634bffd181244e3657b9fe51f8046f296ef2f` |

The scene bytes store little-endian affine axes at `0x27c`, `0x294`, and `0x2ac`, plus translation at `0x2c4`. Terrain extent uses big-endian fields at `0x308`, `0x310`, and `0x318`. Import applies both to the height grid and shifts its centered vertical origin to match the Rust heightfield convention. It maps the scene into the renderer's coordinate system using a common translation and scale. Sphere and cube bounds were checked in x86 disassembly at `0x0044be30` and `0x004a3a70`. The `brycedocset` auxiliary record carries render dimensions at `+8` and `+12`. Those dimensions agree with FIORD, ABISKO, and ETERNAL's original renders.

All 74 disc `.BRC` files rendered in a corpus smoke test. The importer recovered 308 placed heightfields in 64 scenes and all 1,279 sphere and 285 cube records. For example, FIORD restores all seven heightfields and opens with its 780 × 460 framing. Comparison with the disc renders constrains composition and terrain placement. Shading uses independent defaults. The importer uses the highest horizontal plane at or below the camera as the reflective surface. Additional planes, scene hierarchy, Boolean operations, other geometry, the original atmosphere and material layers still need compatibility work. The sample directories expose 335 triangle meshes in 18 scenes, 312 groups in 35 scenes, and material records in all 74 scenes.

## Full-compatibility targets

| Area | Supported now | Remaining for Bryce 2 parity |
| --- | --- | --- |
| Terrain | TIFF and BRC elevation data, multiple placed terrain objects, procedural generation, painting, smoothing | Symmetrical lattices, erosion and other filters, terrain gradients and height-dependent materials |
| Scene | Basic primitives, OBJ meshes, image panels, BRC transforms for supported classes, and BRC perspective cameras | Other BRC geometry and camera types, group nesting, Boolean operations with cut-surface materials, DXF and 3DMF import |
| Materials | Independent solid, checker, marble, lava and ripple patterns, gloss and reflection | Recover original procedural material graphs, image mapping, bump and transparency/refraction, material preset libraries |
| Atmosphere | A sun, cloud layer, gradient sky, and distance haze | Local lights and light gels, visible light volumes, original sky and fog models and presets |
| Rendering | CPU ray tracing, reflections, hard shadows, antialiasing, PNG export, and saved BRC render dimensions | Match shading and effects in the legacy sample renders, reproduce additional original render modes |

The original technical notes in `TECHNOTE.WRI` and installer `readme#2.wri` document several of the missing optics, terrain, lighting, and Boolean features. The disc includes rendered reference images for comparing composition and appearance.
