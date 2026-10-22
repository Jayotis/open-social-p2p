# Open Social P2P: Android app

The phone app is a remote control for your home node ([ADR 0001](../docs/decisions/0001-home-node-architecture.md)). It pairs with the node by QR code and then talks only to it ([ADR 0008](../docs/decisions/0008-device-pairing-and-client-protocol.md)). With it you can read your feed, play videos, post photos and videos, reshare, follow people by their QR code and see whether your node is healthy.

All networking is Rust: [`crates/osp-client`](../crates/osp-client/), reached through [flutter_rust_bridge](https://cjycode.com/flutter_rust_bridge/). This directory holds the Flutter UI (`lib/`) and a thin bridge crate (`rust/`).

| Path | What it is |
| --- | --- |
| `lib/node.dart` | The app's view of its home node: pairing, reachability, health, feed |
| `lib/screens/`, `lib/widgets/` | The UI |
| `lib/src/rust/` | Generated Dart bindings (don't edit) |
| `rust/src/api/osp.rs` | The functions Dart calls |
| `rust/src/frb_generated.rs` | Generated Rust side of the bindings (don't edit) |
| `rust_builder/` | Cargokit, which builds the Rust library during the Flutter build (generated) |

## Build

Requirements: Flutter 3.44 or newer, the Android SDK with NDK 28.2 (Android Studio's SDK manager, or `sdkmanager "ndk;28.2.13676358"`), Rust with the Android target, and Java 17 or newer.

```sh
rustup target add aarch64-linux-android      # most phones; add armv7-linux-androideabi for old ones
cd app
flutter build apk --release --target-platform android-arm64
adb install -r build/app/outputs/flutter-apk/app-release.apk
```

Release builds are signed with the debug key for now. That's fine for testing, not for publishing.

To install without a USB cable, copy the APK to the phone and open it, allowing installs from that source.

## After changing the Rust API

```sh
cargo install flutter_rust_bridge_codegen --version 2.13.0 --locked   # once
cd app && flutter_rust_bridge_codegen generate
```

The generated files are committed, so the app builds without the code generator.

## Checks

```sh
cd app
flutter analyze
flutter test
```

The Rust side is checked with the rest of the workspace (`cargo clippy`, `cargo test` at the repository root). See the [M2 testing guide](../docs/testing/m2-phone.md) for testing on a phone.

## Notes

- The device key, the pairing and the saved feed live in the app's private storage. Android backups are turned off, so the key never leaves the phone; a new phone pairs again.
- The video player loads media from `http://127.0.0.1:<port>/<token>/...`, served by the Rust side from the home node. The network security config allows plain HTTP only to 127.0.0.1.
- The app's log is under *Home node status → Diagnostics log*.
