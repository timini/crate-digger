# 0002: Dependencies and their licences

Status: recorded for milestones 1 and 2. The code licence for Crate Digger itself is still to be chosen (#21); this record is the input for that decision.

## Runtime dependencies

| Crate | Purpose | Licence |
| --- | --- | --- |
| tauri 2 | Desktop shell | MIT OR Apache-2.0 |
| rusqlite (bundled SQLite) | Database | MIT; SQLite is public domain |
| symphonia 0.6 | Audio decoding | MPL-2.0 |
| cpal 0.18 | Audio output | Apache-2.0 |
| rtrb | Lock-free ring buffer for playback | MIT OR Apache-2.0 |
| rubato 5 | Resampling | MIT OR Apache-2.0 |
| lofty | Reading tags | MIT OR Apache-2.0 |
| rusty-chromaprint 0.3 | Audio fingerprints | MIT (see below) |
| realfft | FFT for analysis | MIT |
| ebur128 | Loudness | MIT |
| tract-onnx 0.23 | Running pretrained models | MIT OR Apache-2.0 |
| blake3 | Content hashes | CC0-1.0 OR Apache-2.0 |
| sha2 | Model checksums | MIT OR Apache-2.0 |
| ureq 3 | Downloading models on request; service connectors | MIT OR Apache-2.0 |
| keyring 3.6.3 | OS credential store (Keychain, Windows Credential Manager, Secret Service) | MIT OR Apache-2.0 |
| url 2 | Endpoint validation in connectors | MIT OR Apache-2.0 |
| walkdir, uuid, serde, unicode-normalization | Utilities | MIT OR Apache-2.0 (walkdir also Unlicense) |

Test-only: fail (Apache-2.0), tempfile, vitest and testing-library (MIT).

## Points to settle before release

- **symphonia (MPL-2.0).** File-level copyleft: changes to symphonia's own files must be shared; using it as a library imposes nothing on Crate Digger's code. Compatible with any licence choice.
- **rusty-chromaprint (MIT).** A Rust reimplementation of Chromaprint, whose C++ original is LGPL-2.1. The port is published as MIT and includes Chromaprint's classifier coefficients. Before release, confirm with the port's author that the MIT grant covers those coefficients, or switch to linking the LGPL original.
- **Pretrained models.** Not dependencies of the code. They are downloaded only when the user asks. The Essentia models are CC BY-NC-SA 4.0 (see [0001](0001-analysis-model.md)), which allows use in this free app but not in commercial products or services.
