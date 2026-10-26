import 'dart:io';

/// Keeps the files of a post being sent in the app's own folder until the
/// home node has published it or the owner discards it.
///
/// image_picker leaves its copies of picked photos and videos in the cache
/// folder, which Android may clear whenever it needs room. A post that
/// carries on after the app was stopped needs them for up to a day.
class UploadsFolder {
  UploadsFolder(this.dir, {required this.pickerDir});

  /// One folder per post, with a folder per file in it, so that files keep
  /// their names (the node goes by their extension).
  final Directory dir;

  /// Where image_picker leaves its copies. Files there, or already in
  /// [dir], are moved, which takes no time on the same disk. Others are
  /// copied, so the app never moves a file it doesn't own.
  final Directory pickerDir;

  /// Moves or copies [files] into a new folder for one post, and returns
  /// their new paths in the same order.
  Future<List<String>> take(List<String> files) async {
    await dir.create(recursive: true);
    final post = (await dir.createTemp('post')).path;
    final taken = <String>[];
    for (final (i, path) in files.indexed) {
      final folder = await Directory('$post/$i').create();
      taken.add(await _place(File(path), '${folder.path}/${_name(path)}'));
    }
    return taken;
  }

  /// Deletes the folder of the post with [files], once it was published or
  /// discarded.
  Future<void> release(List<String> files) async {
    for (final post in _posts(files)) {
      await _delete(Directory(post));
    }
  }

  /// Deletes every post folder except the one with [keep], the files of
  /// the post still waiting to be sent. What's left belongs to posts the
  /// app stopped before it could tidy up.
  Future<void> clean({List<String> keep = const []}) async {
    if (!await dir.exists()) return;
    final kept = _posts(keep);
    await for (final entry in dir.list()) {
      if (!kept.contains(entry.path)) await _delete(entry);
    }
  }

  Future<String> _place(File file, String target) async {
    if (!_owned(file.path)) return (await file.copy(target)).path;
    try {
      await file.rename(target);
    } on FileSystemException {
      // Not on the same disk after all.
      await file.copy(target);
      await file.delete();
    }
    await _tidy(file.parent);
    return target;
  }

  bool _owned(String path) =>
      _inside(path, pickerDir.path) || _inside(path, dir.path);

  /// Deletes [folder] and the folders it's in while they're empty, up to
  /// [pickerDir] or [dir].
  Future<void> _tidy(Directory folder) async {
    while (_owned(folder.path)) {
      try {
        await folder.delete();
      } on FileSystemException {
        return; // Not empty.
      }
      folder = folder.parent;
    }
  }

  /// The post folders that [files] are in.
  Set<String> _posts(List<String> files) => {
    for (final path in files)
      if (_inside(path, dir.path))
        '${dir.path}/${path.substring(dir.path.length + 1).split('/').first}',
  };

  static Future<void> _delete(FileSystemEntity entry) async {
    try {
      await entry.delete(recursive: true);
    } on FileSystemException {
      // Gone already.
    }
  }

  static bool _inside(String path, String folder) =>
      path.startsWith('$folder/');

  static String _name(String path) {
    final name = path.split('/').last;
    return name.isEmpty ? 'file' : name;
  }
}
