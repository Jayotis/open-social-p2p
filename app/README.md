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

Without a release key (below), release builds are signed with the debug key and the build prints a warning. That's fine for testing, not for publishing.

To install without a USB cable, copy the APK to the phone and open it, allowing installs from that source.

## Release signing

Release builds are signed with the key that `android/key.properties` names. The key stays on the maintainer's computer; CI doesn't build or sign APKs, so no signing secret is stored on GitHub.

To set it up once, create a keystore outside the repository:

```sh
mkdir -p ~/keys && chmod 700 ~/keys
keytool -genkeypair -keystore ~/keys/osp-release.jks -storetype PKCS12 \
  -keyalg RSA -keysize 4096 -validity 10000 -alias release
```

Then create `android/key.properties` (git ignores it) with the keystore's full path and the password you chose. A PKCS12 keystore uses the same password for the store and the key:

```properties
storeFile=/home/you/keys/osp-release.jks
storePassword=the password
keyAlias=release
keyPassword=the password
```

Back up the keystore and its password somewhere other than this computer. Android only installs an update signed with the same key as the installed app; if the key is lost, people must uninstall the app, losing its pairing, before they can install one signed with a new key. The same applies when switching a phone from a debug-signed build to a release-signed one.

To check which key signed an APK: `apksigner verify --print-certs build/app/outputs/flutter-apk/app-release.apk` (`apksigner` is in the Android SDK's `build-tools`).

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
