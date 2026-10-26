import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/uploads.dart';

void main() {
  late Directory root;
  late UploadsFolder uploads;

  /// A file as image_picker leaves it: `cache/<random>/<name>`.
  File picked(String name, {String folder = 'a1b2'}) {
    final file = File('${root.path}/cache/$folder/$name');
    file.parent.createSync(recursive: true);
    file.writeAsStringSync('bytes of $name');
    return file;
  }

  setUp(() {
    root = Directory.systemTemp.createTempSync('uploads_test');
    uploads = UploadsFolder(
      Directory('${root.path}/files/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    );
  });
  tearDown(() => root.deleteSync(recursive: true));

  test(
    "picked files move into the app's folder, keeping their names",
    () async {
      final a = picked('IMG_1.jpg');
      final b = picked('IMG_1.jpg', folder: 'c3d4');
      final taken = await uploads.take([a.path, b.path]);
      expect(taken, hasLength(2));
      expect(
        taken[0],
        isNot(taken[1]),
        reason: 'same names, different folders',
      );
      for (final path in taken) {
        expect(path, startsWith('${uploads.dir.path}/'));
        expect(path, endsWith('/IMG_1.jpg'));
        expect(File(path).readAsStringSync(), 'bytes of IMG_1.jpg');
      }
      expect(a.existsSync() || b.existsSync(), isFalse, reason: 'moved');
      // The picker's empty folders go too, but not its own.
      expect(Directory('${root.path}/cache').listSync(), isEmpty);
    },
  );

  test("files the app doesn't own are copied, not moved", () async {
    final own = File('${root.path}/Pictures/mine.jpg')
      ..parent.createSync(recursive: true)
      ..writeAsStringSync('original');
    final taken = await uploads.take([own.path]);
    expect(own.readAsStringSync(), 'original');
    expect(File(taken.single).readAsStringSync(), 'original');
  });

  test('posting again moves the files to a new post folder', () async {
    final first = await uploads.take([picked('clip.mp4').path]);
    final second = await uploads.take(first);
    expect(File(second.single).readAsStringSync(), 'bytes of clip.mp4');
    expect(uploads.dir.listSync(), hasLength(1), reason: 'the old one is gone');
  });

  test("a post's folder goes once it's published or discarded", () async {
    final kept = await uploads.take([picked('a.jpg').path]);
    final done = await uploads.take([
      picked('b.jpg').path,
      picked('c.mp4').path,
    ]);
    await uploads.release(done);
    expect(File(kept.single).existsSync(), isTrue);
    expect(uploads.dir.listSync(), hasLength(1));
  });

  test(
    "at start, only the folder of the post waiting to be sent stays",
    () async {
      final waiting = await uploads.take([picked('a.jpg').path]);
      await uploads.take([picked('b.jpg').path]);
      await uploads.take([picked('c.jpg').path]);
      await uploads.clean(keep: waiting);
      expect(File(waiting.single).existsSync(), isTrue);
      expect(uploads.dir.listSync(), hasLength(1));

      await uploads.clean();
      expect(uploads.dir.listSync(), isEmpty);
    },
  );

  test('cleaning an uploads folder that was never made does nothing', () async {
    await uploads.clean(keep: ['${root.path}/cache/x/a.jpg']);
    expect(uploads.dir.existsSync(), isFalse);
  });
}
