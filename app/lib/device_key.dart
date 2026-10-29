import 'dart:io';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Where secrets are kept. On a phone that is its secure storage; tests
/// use a map.
abstract class SecretStore {
  Future<String?> read(String name);
  Future<void> write(String name, String value);
  Future<void> delete(String name);
}

/// The phone's secure storage. On Android it encrypts what it keeps with
/// a key held in the Keystore, which can't be copied off the phone; on an
/// iPhone it uses the Keychain.
class PhoneSecretStore implements SecretStore {
  const PhoneSecretStore();

  static const _storage = FlutterSecureStorage(
    // Not in the iPhone's backups, so it can't move to another phone, as on
    // Android. Readable once the phone was unlocked after starting, so the
    // app could start in the background.
    iOptions: IOSOptions(
      accessibility: KeychainAccessibility.first_unlock_this_device,
    ),
  );

  @override
  Future<String?> read(String name) => _storage.read(key: name);

  @override
  Future<void> write(String name, String value) =>
      _storage.write(key: name, value: value);

  @override
  Future<void> delete(String name) => _storage.delete(key: name);
}

/// The phone's device key, as 64 hex digits.
class DeviceKey {
  const DeviceKey(this.hex, {this.isNew = false});

  final String hex;

  /// Made on this start, because there was none. On a phone that is
  /// already paired, the old key was lost: secure storage clears itself
  /// when it can't decrypt, which happens after some resets.
  final bool isNew;
}

const _name = 'device_key';

/// Loads the device key from [store], making one with [newKey] the first
/// time. A key an older version of the app left as a plain file in [dir]
/// is moved into [store], and the file deleted.
Future<DeviceKey> loadDeviceKey(
  SecretStore store,
  Directory dir,
  String Function() newKey,
) async {
  final oldFile = File('${dir.path}/device.key');
  String? key;
  try {
    key = await store.read(_name);
  } catch (_) {
    // Unreadable, so as good as gone.
    await store.delete(_name);
  }
  if (key == null && await oldFile.exists()) {
    key = (await oldFile.readAsString()).trim();
    await store.write(_name, key);
    // The file goes only once the key reads back from the store.
    if (await store.read(_name) != key) {
      throw StateError("the device key couldn't be moved to secure storage");
    }
  }
  final isNew = key == null;
  if (key == null) {
    key = newKey();
    await store.write(_name, key);
  }
  if (await oldFile.exists()) await oldFile.delete();
  return DeviceKey(key, isNew: isNew);
}
