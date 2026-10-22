import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/device_key.dart';

/// Secure storage for tests: a map, which can be made to fail to read, as
/// the phone's does when the key protecting it is gone.
class FakeStore implements SecretStore {
  final values = <String, String>{};
  bool unreadable = false;

  @override
  Future<String?> read(String name) async {
    if (unreadable) throw Exception('Failed to unwrap key');
    return values[name];
  }

  @override
  Future<void> write(String name, String value) async => values[name] = value;

  @override
  Future<void> delete(String name) async {
    values.remove(name);
    unreadable = false;
  }
}

void main() {
  late Directory dir;
  late File oldFile;
  var made = 0;
  String newKey() => 'new-key-${++made}';

  setUp(() {
    dir = Directory.systemTemp.createTempSync('device_key_test');
    oldFile = File('${dir.path}/device.key');
  });
  tearDown(() => dir.deleteSync(recursive: true));

  test('the first start makes a key and keeps it in the store', () async {
    final store = FakeStore();
    final key = await loadDeviceKey(store, dir, newKey);
    expect(store.values['device_key'], key.hex);
    expect(key.isNew, isTrue);
    expect(oldFile.existsSync(), isFalse);
    // Later starts get the same key.
    final again = await loadDeviceKey(store, dir, newKey);
    expect(again.hex, key.hex);
    expect(again.isNew, isFalse);
  });

  test("an older version's key file moves into the store", () async {
    oldFile.writeAsStringSync('abcd1234\n');
    final store = FakeStore();
    final key = await loadDeviceKey(store, dir, newKey);
    expect(key.hex, 'abcd1234', reason: 'the same key, so still paired');
    expect(key.isNew, isFalse);
    expect(store.values['device_key'], 'abcd1234');
    expect(oldFile.existsSync(), isFalse);
  });

  test('a file left behind after moving is deleted', () async {
    final store = FakeStore()..values['device_key'] = 'kept';
    oldFile.writeAsStringSync('kept');
    final key = await loadDeviceKey(store, dir, newKey);
    expect(key.hex, 'kept');
    expect(oldFile.existsSync(), isFalse);
  });

  test('a key that can no longer be read is replaced, and says so', () async {
    final store = FakeStore()
      ..values['device_key'] = 'lost'
      ..unreadable = true;
    final key = await loadDeviceKey(store, dir, newKey);
    expect(key.isNew, isTrue);
    expect(key.hex, isNot('lost'));
    expect(store.values['device_key'], key.hex);
  });

  test('a store that cleared itself gets a new key, and says so', () async {
    // What the phone's secure storage does when it can't decrypt.
    final store = FakeStore()..values['device_key'] = 'lost';
    await loadDeviceKey(store, dir, newKey);
    store.values.clear();
    final key = await loadDeviceKey(store, dir, newKey);
    expect(key.isNew, isTrue);
    expect(store.values['device_key'], key.hex);
  });
}
